//! Forward-confirmed reverse DNS (spec 5.1, step 3).
//!
//! An address is confirmed as an operator's when its reverse name ends in one
//! of the operator's suffixes **and** that name resolves back to the same
//! address. The second half matters: anyone who controls the reverse zone for
//! their own addresses can make them claim to be `crawl-1.googlebot.com`, but
//! they cannot make Google's zone point back at them.
//!
//! This is the only way to verify bots whose operators publish no IP list,
//! and a second chance for an address outside a published list. Lookups are
//! optional, run once per address, and are cached on disk.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::NaiveDate;
use duckdb::params;
use serde::{Deserialize, Serialize};

use super::registry::Registry;
use crate::error::Result;
use crate::ingest::rebuild_rollups;
use crate::store::Store;

/// Cached answers are trusted for this long.
const CACHE_DAYS: i64 = 30;
/// A negative answer says nothing about who held the address long ago, so it
/// only makes requests from the last this-many days fake.
const NEGATIVE_WINDOW_DAYS: i64 = 30;
/// New lookups per run. The rest stay unverifiable until the next run.
const MAX_LOOKUPS: usize = 5_000;
const WORKERS: usize = 64;

/// The result of one DNS question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup<T> {
    Found(T),
    /// The name server answered: there is no such record.
    Missing,
    /// No usable answer (timeout, server failure, no network). Not a negative.
    Failed,
}

pub trait Resolver: Sync {
    /// The reverse (PTR) name of an address.
    fn reverse(&self, ip: IpAddr) -> Lookup<String>;
    /// The addresses a name resolves to.
    fn forward(&self, host: &str) -> Lookup<Vec<IpAddr>>;
}

/// "No such name" is an answer; anything else (timeout, server failure, no
/// network) is not, and must not be read as a negative.
fn missing_or_failed<T>(error: &dns_lookup::LookupError) -> Lookup<T> {
    use dns_lookup::LookupErrorKind::{NoData, NoName};
    match error.kind() {
        NoName | NoData => Lookup::Missing,
        _ => Lookup::Failed,
    }
}

/// Uses the operating system's resolver.
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn reverse(&self, ip: IpAddr) -> Lookup<String> {
        match dns_lookup::lookup_addr(&ip) {
            // With no reverse record the system hands the address back as text.
            Ok(name) if name.parse::<IpAddr>().is_ok() => Lookup::Missing,
            Ok(name) => Lookup::Found(name),
            Err(e) => missing_or_failed(&e),
        }
    }

    fn forward(&self, host: &str) -> Lookup<Vec<IpAddr>> {
        match dns_lookup::lookup_host(host) {
            Ok(addresses) => {
                let addresses: Vec<IpAddr> = addresses.map(|a| a.to_canonical()).collect();
                if addresses.is_empty() {
                    Lookup::Missing
                } else {
                    Lookup::Found(addresses)
                }
            }
            Err(e) => missing_or_failed(&e),
        }
    }
}

/// What is known about one address's reverse DNS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The reverse name, lowercase, without a trailing dot. `None` when the
    /// address has no reverse record.
    pub name: Option<String>,
    /// The reverse name resolves back to the address.
    pub confirmed: bool,
    pub checked: NaiveDate,
}

impl Entry {
    /// True when the address is confirmed as belonging under one of `suffixes`.
    pub fn matches(&self, suffixes: &[String]) -> bool {
        let Some(name) = &self.name else {
            return false;
        };
        self.confirmed
            && suffixes.iter().any(|suffix| {
                let suffix = suffix.trim_matches('.').to_ascii_lowercase();
                name.strip_suffix(&suffix)
                    .is_some_and(|head| head.is_empty() || head.ends_with('.'))
            })
    }
}

/// Asks the resolver about one address. `None` when no usable answer came back.
pub fn confirm(resolver: &dyn Resolver, ip: IpAddr, today: NaiveDate) -> Option<Entry> {
    let name = match resolver.reverse(ip) {
        Lookup::Found(name) => name.trim_end_matches('.').to_ascii_lowercase(),
        Lookup::Missing => {
            return Some(Entry {
                name: None,
                confirmed: false,
                checked: today,
            });
        }
        Lookup::Failed => return None,
    };
    let confirmed = match resolver.forward(&name) {
        Lookup::Found(addresses) => addresses
            .iter()
            .any(|a| a.to_canonical() == ip.to_canonical()),
        Lookup::Missing => false,
        Lookup::Failed => return None,
    };
    Some(Entry {
        name: Some(name),
        confirmed,
        checked: today,
    })
}

/// The on-disk cache, shared by every site in the data directory.
pub struct DnsCache {
    path: PathBuf,
    entries: HashMap<String, Entry>,
}

impl DnsCache {
    pub fn load(data_root: &Path) -> Self {
        let path = data_root.join("ranges").join("reverse-dns.json");
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        DnsCache { path, entries }
    }

    fn fresh(&self, ip: &str, today: NaiveDate) -> Option<&Entry> {
        self.entries
            .get(ip)
            .filter(|e| today.signed_duration_since(e.checked).num_days() <= CACHE_DAYS)
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = self.path.with_extension("json.tmp");
        std::fs::write(
            &temp,
            serde_json::to_string(&self.entries).expect("cache serialises"),
        )?;
        std::fs::rename(&temp, &self.path)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct DnsOutcome {
    /// Addresses that needed a reverse DNS check.
    pub addresses: u64,
    /// Of those, looked up now (the rest came from the cache).
    pub looked_up: u64,
    /// Lookups that got no usable answer; these stay as they were.
    pub failed: u64,
    /// Left for the next run because of the per-run limit.
    pub deferred: u64,
    /// Addresses confirmed as the bot they claim to be.
    pub confirmed: u64,
}

/// Runs forward-confirmed reverse DNS for every address that claims a bot
/// whose operator has reverse DNS suffixes and is not already verified, then
/// updates the stored verdicts and the rollups.
///
/// - Confirmed: the request is `verified`, whatever the IP ranges said.
/// - Not confirmed, for an operator with no published ranges: `fake`, for
///   requests in the last 30 days; older ones stay `unverifiable`.
/// - Not confirmed, for an operator with ranges: unchanged. The ranges have
///   already decided.
/// - No usable answer: unchanged.
pub fn apply(
    store: &Store,
    registry: &Registry,
    resolver: &dyn Resolver,
    today: NaiveDate,
) -> Result<DnsOutcome> {
    let conn = &store.conn;

    // Bot key → (suffixes, operator publishes ranges).
    let mut bots: HashMap<String, (Vec<String>, bool)> = HashMap::new();
    for (operator, bot) in registry.bots() {
        if !operator.rdns.is_empty() {
            bots.insert(
                bot.key.clone(),
                (operator.rdns.clone(), !operator.range_urls.is_empty()),
            );
        }
    }
    if bots.is_empty() {
        return Ok(DnsOutcome::default());
    }
    let keys = bots
        .keys()
        .map(|k| crate::report::quote(k))
        .collect::<Vec<_>>()
        .join(", ");

    let pairs: Vec<(String, String)> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT DISTINCT r.ip, u.bot_key FROM requests r JOIN user_agents u USING (ua_id)
             WHERE u.bot_key IN ({keys}) AND (r.bot_verify IS NULL OR r.bot_verify <> 'verified')"
        ))?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<duckdb::Result<_>>()?
    };
    let addresses: BTreeSet<&str> = pairs.iter().map(|(ip, _)| ip.as_str()).collect();
    let mut outcome = DnsOutcome {
        addresses: addresses.len() as u64,
        ..DnsOutcome::default()
    };

    // Look up what the cache does not already know, a batch at a time.
    let mut cache = DnsCache::load(store.data_root());
    let wanted: Vec<IpAddr> = addresses
        .iter()
        .filter(|ip| cache.fresh(ip, today).is_none())
        .filter_map(|ip| ip.parse().ok())
        .collect();
    outcome.deferred = wanted.len().saturating_sub(MAX_LOOKUPS) as u64;
    let batch = &wanted[..wanted.len().min(MAX_LOOKUPS)];
    if !batch.is_empty() {
        let next = AtomicUsize::new(0);
        let found: Mutex<Vec<(IpAddr, Option<Entry>)>> =
            Mutex::new(Vec::with_capacity(batch.len()));
        std::thread::scope(|scope| {
            for _ in 0..WORKERS.min(batch.len()) {
                scope.spawn(|| {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(ip) = batch.get(index) else { break };
                        let entry = confirm(resolver, *ip, today);
                        found
                            .lock()
                            .expect("no panics hold this lock")
                            .push((*ip, entry));
                    }
                });
            }
        });
        for (ip, entry) in found.into_inner().expect("workers have finished") {
            outcome.looked_up += 1;
            match entry {
                Some(entry) => {
                    cache.entries.insert(ip.to_string(), entry);
                }
                None => outcome.failed += 1,
            }
        }
        cache.save()?;
    }

    // Turn the answers into verdicts per address and bot.
    let negative_from = super::ranges::day_number(today) - NEGATIVE_WINDOW_DAYS;
    let mut confirmed_ips = HashSet::new();
    conn.execute_batch("BEGIN TRANSACTION")?;
    let result = (|| -> Result<()> {
        conn.execute_batch(
            "CREATE OR REPLACE TEMP TABLE loga_dns
             (ip VARCHAR, bot_key VARCHAR, confirmed BOOLEAN, decides BOOLEAN)",
        )?;
        {
            let mut appender = conn.appender("loga_dns")?;
            for (ip, bot_key) in &pairs {
                let (Some(entry), Some((suffixes, has_ranges))) =
                    (cache.fresh(ip, today), bots.get(bot_key))
                else {
                    continue;
                };
                let confirmed = entry.matches(suffixes);
                if confirmed {
                    confirmed_ips.insert(ip.as_str());
                }
                // A negative only decides when there are no ranges to go by.
                appender.append_row(params![ip, bot_key, confirmed, confirmed || !has_ranges])?;
            }
            appender.flush()?;
        }
        let days: BTreeSet<i64> = {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT datediff('day', DATE '1970-01-01', CAST(r.ts_local AS DATE))
                 FROM requests r JOIN user_agents u USING (ua_id)
                 JOIN loga_dns t ON t.ip = r.ip AND t.bot_key = u.bot_key WHERE t.decides",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
            rows.collect::<duckdb::Result<_>>()?
        };
        conn.execute_batch(&format!(
            "UPDATE requests SET bot_verify = CASE
                 WHEN t.confirmed THEN 'verified'
                 WHEN datediff('day', DATE '1970-01-01', CAST(requests.ts AS DATE)) >= {negative_from} THEN 'fake'
                 ELSE 'unverifiable' END
             FROM loga_dns t, user_agents u
             WHERE t.decides AND requests.ip = t.ip AND requests.ua_id = u.ua_id AND u.bot_key = t.bot_key
               AND (requests.bot_verify IS NULL OR requests.bot_verify <> 'verified');"
        ))?;
        let pruned_before = store
            .meta("pruned_before")?
            .and_then(|d| d.parse::<NaiveDate>().ok());
        rebuild_rollups(conn, &days, pruned_before)?;
        Ok(())
    })();
    conn.execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
    result?;
    outcome.confirmed = confirmed_ips.len() as u64;
    Ok(outcome)
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// A resolver with fixed answers. Anything not listed fails.
    #[derive(Default)]
    pub struct FakeResolver {
        pub reverse: HashMap<IpAddr, Lookup<String>>,
        pub forward: HashMap<String, Lookup<Vec<IpAddr>>>,
    }

    impl FakeResolver {
        /// An address with a reverse name that resolves back to it.
        pub fn genuine(mut self, ip: &str, name: &str) -> Self {
            let ip: IpAddr = ip.parse().unwrap();
            self.reverse.insert(ip, Lookup::Found(name.to_owned()));
            self.forward.insert(
                name.trim_end_matches('.').to_ascii_lowercase(),
                Lookup::Found(vec![ip]),
            );
            self
        }

        /// An address whose owner made its reverse name claim to be `name`,
        /// which really resolves somewhere else.
        pub fn spoofed(mut self, ip: &str, name: &str, real: &str) -> Self {
            self.reverse
                .insert(ip.parse().unwrap(), Lookup::Found(name.to_owned()));
            self.forward.insert(
                name.trim_end_matches('.').to_ascii_lowercase(),
                Lookup::Found(vec![real.parse().unwrap()]),
            );
            self
        }

        pub fn no_record(mut self, ip: &str) -> Self {
            self.reverse.insert(ip.parse().unwrap(), Lookup::Missing);
            self
        }
    }

    impl Resolver for FakeResolver {
        fn reverse(&self, ip: IpAddr) -> Lookup<String> {
            self.reverse.get(&ip).cloned().unwrap_or(Lookup::Failed)
        }
        fn forward(&self, host: &str) -> Lookup<Vec<IpAddr>> {
            self.forward.get(host).cloned().unwrap_or(Lookup::Failed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeResolver;
    use super::*;

    fn day(text: &str) -> NaiveDate {
        text.parse().unwrap()
    }

    fn suffixes(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn forward_confirmation_is_required() {
        let today = day("2026-11-16");
        let resolver = FakeResolver::default()
            .genuine("192.0.2.10", "crawl-1.Googlebot.com.")
            .spoofed("192.0.2.66", "crawl-9.googlebot.com", "192.0.2.10")
            .genuine("192.0.2.20", "host.example.net")
            .no_record("192.0.2.30");
        let google = suffixes(&["googlebot.com", "google.com"]);
        let check = |ip: &str| confirm(&resolver, ip.parse().unwrap(), today);

        let genuine = check("192.0.2.10").unwrap();
        assert_eq!(genuine.name.as_deref(), Some("crawl-1.googlebot.com"));
        assert!(genuine.matches(&google));

        // The reverse name says Googlebot, but Google's zone does not point back.
        let spoofed = check("192.0.2.66").unwrap();
        assert!(!spoofed.confirmed && !spoofed.matches(&google));

        // A real, confirmed name that is simply not Google's.
        let other = check("192.0.2.20").unwrap();
        assert!(other.confirmed && !other.matches(&google));

        let none = check("192.0.2.30").unwrap();
        assert!(none.name.is_none() && !none.matches(&google));

        // No answer at all is not a negative.
        assert_eq!(check("192.0.2.99"), None);
    }

    #[test]
    fn suffixes_match_whole_labels_only() {
        let entry = |name: &str| Entry {
            name: Some(name.to_owned()),
            confirmed: true,
            checked: day("2026-11-16"),
        };
        let google = suffixes(&["googlebot.com"]);
        assert!(entry("crawl-1.googlebot.com").matches(&google));
        assert!(entry("googlebot.com").matches(&google));
        assert!(!entry("evilgooglebot.com").matches(&google));
        assert!(!entry("googlebot.com.evil.example").matches(&google));
    }
}

#[cfg(test)]
mod apply_tests {
    use std::sync::atomic::AtomicBool;

    use super::testing::FakeResolver;
    use super::*;
    use crate::ingest::{IngestOptions, ingest};
    use crate::store::testing::TempDataDir;
    use crate::store::{OpenOptions, SiteSettings};

    const AMAZON: &str = "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; Amazonbot/0.1; +https://developer.amazon.com/support/amazonbot) Chrome/119.0.6045.214 Safari/537.36";
    const GOOGLE: &str = "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";

    fn line(ip: &str, date: &str, ua: &str) -> String {
        format!("{ip} - - [{date}:10:00:00 +0000] \"GET / HTTP/2.0\" 200 100 \"-\" \"{ua}\"\n")
    }

    #[test]
    fn reverse_dns_settles_what_ranges_cannot() {
        let tmp = TempDataDir::new("rdns-apply");
        let mut store = Store::create(
            &tmp.0,
            "site",
            &SiteSettings::default(),
            &OpenOptions::default(),
        )
        .unwrap();
        let log = [
            // Amazon publishes no ranges: reverse DNS is the only check.
            line("192.0.2.10", "14/Nov/2026", AMAZON), // genuine
            line("192.0.2.66", "14/Nov/2026", AMAZON), // no reverse record
            line("192.0.2.67", "14/Nov/2026", AMAZON), // reverse name claims Amazon, forward disagrees
            line("192.0.2.66", "01/Aug/2026", AMAZON), // same impostor, long ago
            line("192.0.2.99", "14/Nov/2026", AMAZON), // resolver gives no answer
            // No Google ranges are cached here, so Googlebot starts unverifiable.
            line("192.0.2.20", "14/Nov/2026", GOOGLE), // genuine by reverse DNS
        ]
        .concat();
        let path = tmp.0.root().join("access.log");
        std::fs::write(&path, log).unwrap();
        ingest(
            &mut store,
            &[path],
            &IngestOptions::default(),
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();

        let resolver = FakeResolver::default()
            .genuine("192.0.2.10", "ec2-1.crawl.amazonbot.amazon")
            .no_record("192.0.2.66")
            .spoofed("192.0.2.67", "ec2-9.crawl.amazonbot.amazon", "192.0.2.10")
            .genuine("192.0.2.20", "crawl-192-0-2-20.googlebot.com");
        let today: NaiveDate = "2026-11-16".parse().unwrap();
        let outcome = apply(&store, &Registry::embedded(), &resolver, today).unwrap();
        assert_eq!(
            (
                outcome.addresses,
                outcome.looked_up,
                outcome.failed,
                outcome.confirmed
            ),
            (5, 5, 1, 2)
        );

        let verdict = |ip: &str, month: u32| -> Option<String> {
            store
                .conn
                .query_row(
                    "SELECT bot_verify FROM requests WHERE ip = ? AND month(ts) = ?",
                    params![ip, month],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert_eq!(verdict("192.0.2.10", 11).as_deref(), Some("verified"));
        assert_eq!(verdict("192.0.2.66", 11).as_deref(), Some("fake"));
        assert_eq!(verdict("192.0.2.67", 11).as_deref(), Some("fake"));
        // Too long ago for today's DNS to say who held the address.
        assert_eq!(verdict("192.0.2.66", 8).as_deref(), Some("unverifiable"));
        // No answer changes nothing.
        assert_eq!(verdict("192.0.2.99", 11).as_deref(), Some("unverifiable"));
        assert_eq!(verdict("192.0.2.20", 11).as_deref(), Some("verified"));

        // The rollups follow, and a second run answers from the cache.
        let verified: i64 = store
            .conn
            .query_row(
                "SELECT sum(hits) FROM daily_bots WHERE bot_verify = 'verified'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(verified, 2);
        let again = apply(&store, &Registry::embedded(), &resolver, today).unwrap();
        // Only the address with no answer is asked about again.
        assert_eq!(again.looked_up, 1);
    }
}
