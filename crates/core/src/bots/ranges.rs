//! Published IP ranges: fetching, caching with history, and lookups.
//!
//! One JSON file per operator in `<data-dir>/ranges/`. Prefixes are never
//! deleted: one that disappears from the published list is kept with the date
//! it was retired, so older logs are judged against what was published then.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use super::registry::{Operator, Registry};
use crate::error::{Error, Result};

/// Sent on every fetch. At least one list rejects default library agents.
pub const USER_AGENT: &str = "verified-bot-ip-updater/1.0";

/// Lists are refreshed when older than this.
pub const MAX_AGE: Duration = Duration::from_secs(24 * 3600);

/// Ranges change slowly, so a list is trusted to describe this many days
/// before it was first fetched. Earlier requests from outside it are
/// `unverifiable` rather than `fake`.
const GRACE_DAYS: i64 = 30;

/// Successful fetch dates kept per operator.
const MAX_FETCH_LOG: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Verified,
    Fake,
    Unverifiable,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Verified => "verified",
            Verdict::Fake => "fake",
            Verdict::Unverifiable => "unverifiable",
        }
    }
}

/// How one address stands against one operator's ranges. The verdict for a
/// request then depends only on the request's date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assessment {
    /// Inside a currently published prefix.
    pub current: bool,
    /// Inside a retired prefix: verified for requests up to this day
    /// (days since the Unix epoch).
    pub verified_until: Option<i64>,
    /// Outside every prefix: fake for requests from this day on. `None` when
    /// the operator has no ranges to check against.
    pub fake_from: Option<i64>,
}

impl Assessment {
    pub const UNVERIFIABLE: Assessment = Assessment {
        current: false,
        verified_until: None,
        fake_from: None,
    };

    /// `day` is the request's date as days since the Unix epoch (UTC).
    pub fn verdict(&self, day: i64) -> Verdict {
        if self.current || self.verified_until.is_some_and(|until| day <= until) {
            Verdict::Verified
        } else if self.fake_from.is_some_and(|from| day >= from) {
            Verdict::Fake
        } else {
            Verdict::Unverifiable
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredPrefix {
    cidr: String,
    /// The first fetch that contained this prefix.
    first_seen: NaiveDate,
    /// The first fetch that no longer contained it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retired_at: Option<NaiveDate>,
}

/// The on-disk cache for one operator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct RangeFile {
    operator: String,
    /// Date of the first successful fetch.
    tracked_since: NaiveDate,
    /// Time of the latest successful fetch, UTC.
    fetched_at: String,
    /// Dates of successful fetches, oldest first.
    #[serde(default)]
    fetches: Vec<NaiveDate>,
    sources: Vec<String>,
    prefixes: Vec<StoredPrefix>,
}

fn day_number(date: NaiveDate) -> i64 {
    date.signed_duration_since(NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid date"))
        .num_days()
}

fn ip_number(ip: IpAddr) -> u128 {
    match ip.to_canonical() {
        IpAddr::V4(v4) => 0xffff_0000_0000 | u128::from(u32::from(v4)),
        IpAddr::V6(v6) => u128::from(v6),
    }
}

/// Parses `a.b.c.d/n` or `x:y::/n` (or a bare address) into an inclusive
/// range in one number space, with IPv4 mapped into IPv6.
fn parse_cidr(cidr: &str) -> Option<(u128, u128)> {
    let cidr = cidr.trim();
    let (addr, prefix) = match cidr.split_once('/') {
        Some((addr, prefix)) => (addr, Some(prefix.parse::<u32>().ok()?)),
        None => (cidr, None),
    };
    let ip: IpAddr = addr.parse().ok()?;
    let bits = match ip {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    let prefix = prefix.unwrap_or(bits);
    if prefix > bits {
        return None;
    }
    let host_bits = bits - prefix;
    let mask: u128 = if host_bits >= 128 {
        u128::MAX
    } else {
        (1u128 << host_bits) - 1
    };
    let number = match ip {
        IpAddr::V4(v4) => 0xffff_0000_0000 | u128::from(u32::from(v4)),
        IpAddr::V6(v6) => u128::from(v6),
    };
    let start = number & !mask;
    Some((start, start | mask))
}

/// One operator's ranges, ready for lookups.
#[derive(Debug, Clone, Default)]
pub struct RangeSet {
    /// Disjoint and sorted.
    active: Vec<(u128, u128)>,
    /// (start, end, day retired).
    retired: Vec<(u128, u128, i64)>,
    fake_from: Option<i64>,
}

impl RangeSet {
    /// Builds the lookup set from the cached published list (if any) plus
    /// the registry's `extra_ranges`. `None` when there is neither.
    fn build(file: Option<&RangeFile>, extra_ranges: &[String]) -> Option<Self> {
        let mut active: Vec<(u128, u128)> =
            extra_ranges.iter().filter_map(|c| parse_cidr(c)).collect();
        let mut retired = Vec::new();
        if file.is_none() && active.is_empty() {
            return None;
        }
        for prefix in file.map(|f| f.prefixes.as_slice()).unwrap_or_default() {
            let Some((start, end)) = parse_cidr(&prefix.cidr) else {
                continue;
            };
            match prefix.retired_at {
                // Retired on the day of the fetch that missed it, so it was
                // last known good the day before.
                Some(date) => retired.push((start, end, day_number(date) - 1)),
                None => active.push((start, end)),
            }
        }
        active.sort_unstable();
        let mut merged: Vec<(u128, u128)> = Vec::with_capacity(active.len());
        for (start, end) in active {
            match merged.last_mut() {
                Some(last) if start <= last.1.saturating_add(1) => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        // Without a fetched published list nothing can be called fake: the
        // extra ranges only ever confirm.
        let fake_from = file
            .filter(|f| !f.prefixes.is_empty())
            .map(|f| day_number(f.tracked_since) - GRACE_DAYS);
        Some(RangeSet {
            active: merged,
            retired,
            fake_from,
        })
    }

    pub fn assess(&self, ip: IpAddr) -> Assessment {
        let n = ip_number(ip);
        let after = self.active.partition_point(|(start, _)| *start <= n);
        let current = after > 0 && n <= self.active[after - 1].1;
        let verified_until = if current {
            None
        } else {
            self.retired
                .iter()
                .filter(|(start, end, _)| (*start..=*end).contains(&n))
                .map(|(_, _, day)| *day)
                .max()
        };
        Assessment {
            current,
            verified_until,
            fake_from: self.fake_from,
        }
    }
}

/// Every operator's cached ranges, indexed like `Registry::operators()`.
#[derive(Debug, Clone, Default)]
pub struct Ranges {
    sets: Vec<Option<RangeSet>>,
}

fn ranges_dir(data_root: &Path) -> PathBuf {
    data_root.join("ranges")
}

fn range_path(data_root: &Path, operator: &str) -> PathBuf {
    ranges_dir(data_root).join(format!("{operator}.json"))
}

fn read_file(data_root: &Path, operator: &str) -> Option<RangeFile> {
    let text = std::fs::read_to_string(range_path(data_root, operator)).ok()?;
    serde_json::from_str(&text).ok()
}

impl Ranges {
    /// Loads the cache. Works fully offline; operators never fetched have no set.
    pub fn load(data_root: &Path, registry: &Registry) -> Self {
        Ranges {
            sets: registry
                .operators()
                .iter()
                .map(|o| RangeSet::build(read_file(data_root, &o.key).as_ref(), &o.extra_ranges))
                .collect(),
        }
    }

    /// No cached ranges at all.
    pub fn empty(registry: &Registry) -> Self {
        Ranges {
            sets: vec![None; registry.operators().len()],
        }
    }

    pub fn assess(&self, operator: usize, ip: IpAddr) -> Assessment {
        match self.sets.get(operator).and_then(Option::as_ref) {
            Some(set) => set.assess(ip),
            None => Assessment::UNVERIFIABLE,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateOutcome {
    pub operator: String,
    /// False when the previous copy was kept.
    pub updated: bool,
    /// Currently published prefixes after this run.
    pub prefixes: usize,
    pub added: usize,
    pub retired: usize,
    /// Why the previous copy was kept.
    pub message: Option<String>,
}

/// Extracts the prefixes from one published list: the JSON shape most
/// operators use, or the `route:` / `route6:` lines of a whois answer.
fn parse_list(url: &str, text: &str) -> std::result::Result<Vec<String>, String> {
    if url.starts_with("whois://") {
        return parse_whois(text);
    }
    let json: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let entries = json
        .get("prefixes")
        .and_then(|p| p.as_array())
        .ok_or("no \"prefixes\" array")?;
    let mut prefixes = Vec::with_capacity(entries.len());
    for entry in entries {
        let cidr = entry
            .get("ipv4Prefix")
            .or_else(|| entry.get("ipv6Prefix"))
            .and_then(|v| v.as_str())
            .ok_or("a prefix entry has neither ipv4Prefix nor ipv6Prefix")?;
        check_prefix(cidr)?;
        prefixes.push(cidr.trim().to_owned());
    }
    Ok(prefixes)
}

/// A fetched prefix must parse and must not be implausibly broad. No operator
/// publishes anything wider than a /8 (IPv4) or a /16 (IPv6); a list that
/// does has been tampered with or is not what it claims to be, and accepting
/// it would mark most of the internet as a verified bot.
fn check_prefix(cidr: &str) -> std::result::Result<(), String> {
    let (start, end) = parse_cidr(cidr).ok_or_else(|| format!("invalid prefix '{cidr}'"))?;
    let is_v4 = !cidr.contains(':');
    let size = end - start;
    let limit: u128 = if is_v4 { (1 << 24) - 1 } else { (1 << 112) - 1 };
    if size > limit {
        return Err(format!("prefix '{cidr}' is implausibly broad"));
    }
    Ok(())
}

/// Routes announced by an autonomous system, from a routing registry's whois
/// answer. Lines look like `route:      31.13.24.0/21` and `route6:     2a03:2880::/32`.
fn parse_whois(text: &str) -> std::result::Result<Vec<String>, String> {
    let mut prefixes = Vec::new();
    for line in text.lines() {
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        if field != "route" && field != "route6" {
            continue;
        }
        let cidr = value.trim();
        check_prefix(cidr)?;
        prefixes.push(cidr.to_owned());
    }
    if prefixes.is_empty() {
        return Err("the whois answer has no route lines".to_owned());
    }
    Ok(prefixes)
}

/// Fetches one operator's lists and merges them into its cache. The cache is
/// replaced only if every list fetched and parsed and the total passes the
/// operator's minimum; otherwise the previous copy is kept.
fn update_operator(
    data_root: &Path,
    operator: &Operator,
    now: DateTime<Utc>,
    fetch: &dyn Fn(&str) -> std::result::Result<String, String>,
) -> Result<UpdateOutcome> {
    let previous = read_file(data_root, &operator.key);
    let kept = |message: String| UpdateOutcome {
        operator: operator.key.clone(),
        updated: false,
        prefixes: previous.as_ref().map_or(0, |f| {
            f.prefixes.iter().filter(|p| p.retired_at.is_none()).count()
        }),
        added: 0,
        retired: 0,
        message: Some(message),
    };

    let mut published: Vec<String> = Vec::new();
    for url in &operator.range_urls {
        let text = match fetch(url) {
            Ok(text) => text,
            Err(e) => return Ok(kept(format!("{url}: {e}"))),
        };
        match parse_list(url, &text) {
            Ok(list) if list.is_empty() => return Ok(kept(format!("{url}: the list is empty"))),
            Ok(list) => published.extend(list),
            Err(e) => return Ok(kept(format!("{url}: {e}"))),
        }
    }
    published.sort();
    published.dedup();
    if published.len() < operator.min_prefixes.max(1) {
        return Ok(kept(format!(
            "only {} prefixes, fewer than the minimum of {}; refusing the update",
            published.len(),
            operator.min_prefixes
        )));
    }

    let today = now.date_naive();
    let mut prefixes = previous
        .as_ref()
        .map(|f| f.prefixes.clone())
        .unwrap_or_default();
    let (mut added, mut retired) = (0, 0);
    for stored in &mut prefixes {
        let listed = published.binary_search(&stored.cidr).is_ok();
        match (listed, stored.retired_at) {
            (false, None) => {
                stored.retired_at = Some(today);
                retired += 1;
            }
            // Back on the list: active again.
            (true, Some(_)) => {
                stored.retired_at = None;
                added += 1;
            }
            _ => {}
        }
    }
    for cidr in &published {
        if !prefixes.iter().any(|p| &p.cidr == cidr) {
            prefixes.push(StoredPrefix {
                cidr: cidr.clone(),
                first_seen: today,
                retired_at: None,
            });
            added += 1;
        }
    }
    prefixes.sort_by(|a, b| a.cidr.cmp(&b.cidr));

    let mut fetches = previous
        .as_ref()
        .map(|f| f.fetches.clone())
        .unwrap_or_default();
    if fetches.last() != Some(&today) {
        fetches.push(today);
    }
    if fetches.len() > MAX_FETCH_LOG {
        fetches.drain(..fetches.len() - MAX_FETCH_LOG);
    }
    let file = RangeFile {
        operator: operator.key.clone(),
        tracked_since: previous.as_ref().map_or(today, |f| f.tracked_since),
        fetched_at: now.to_rfc3339_opts(SecondsFormat::Secs, true),
        fetches,
        sources: operator.range_urls.clone(),
        prefixes,
    };

    // Write to a temporary file and rename, so a crash cannot leave half a cache.
    std::fs::create_dir_all(ranges_dir(data_root))?;
    let path = range_path(data_root, &operator.key);
    let temp = path.with_extension("json.tmp");
    std::fs::write(
        &temp,
        serde_json::to_string_pretty(&file).expect("range files serialise"),
    )?;
    std::fs::rename(&temp, &path)?;

    Ok(UpdateOutcome {
        operator: operator.key.clone(),
        updated: true,
        prefixes: published.len(),
        added,
        retired,
        message: None,
    })
}

/// Fetches every operator's lists. Operators without lists are skipped.
pub fn update(
    data_root: &Path,
    registry: &Registry,
    now: DateTime<Utc>,
    fetch: &dyn Fn(&str) -> std::result::Result<String, String>,
) -> Result<Vec<UpdateOutcome>> {
    registry
        .operators()
        .iter()
        .filter(|o| !o.range_urls.is_empty())
        .map(|o| update_operator(data_root, o, now, fetch))
        .collect()
}

/// Asks a whois server one question (RFC 3912: send a line, read until the
/// server closes). `source` is `whois://<server>/<query>`.
fn whois_fetch(source: &str) -> std::result::Result<String, String> {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};

    let rest = source
        .strip_prefix("whois://")
        .ok_or("not a whois source")?;
    let (server, query) = rest
        .split_once('/')
        .ok_or("a whois source needs a query: whois://server/query")?;
    let timeout = Duration::from_secs(20);
    let address = (server, 43)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("the whois server has no address")?;
    let mut stream = TcpStream::connect_timeout(&address, timeout).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(format!("{query}\r\n").as_bytes())
        .map_err(|e| e.to_string())?;
    let mut answer = Vec::new();
    // Stop at 8 MB: far more than any operator's route list.
    stream
        .take(8 << 20)
        .read_to_end(&mut answer)
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&answer).into_owned())
}

/// The real fetcher: HTTPS GET with an identifying user agent, or a whois
/// query for `whois://` sources.
pub fn http_fetch(url: &str) -> std::result::Result<String, String> {
    if url.starts_with("whois://") {
        return whois_fetch(url);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(USER_AGENT)
        .build()
        .into();
    let mut response = agent.get(url).call().map_err(|e| e.to_string())?;
    response
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())
}

/// Updates the cache over HTTP.
pub fn update_online(data_root: &Path, registry: &Registry) -> Result<Vec<UpdateOutcome>> {
    update(data_root, registry, Utc::now(), &http_fetch)
}

#[derive(Debug, Clone, Serialize)]
pub struct RangeStatus {
    pub operator: String,
    pub name: String,
    pub sources: usize,
    pub prefixes: usize,
    pub retired: usize,
    pub tracked_since: Option<NaiveDate>,
    pub fetched_at: Option<String>,
    pub fetches: usize,
    /// Older than 24 hours, or never fetched.
    pub stale: bool,
}

fn is_stale(file: Option<&RangeFile>, now: DateTime<Utc>) -> bool {
    file.and_then(|f| DateTime::parse_from_rfc3339(&f.fetched_at).ok())
        .is_none_or(|fetched| {
            now.signed_duration_since(fetched)
                .to_std()
                .is_ok_and(|age| age > MAX_AGE)
        })
}

/// The state of the cache for every operator that publishes ranges.
pub fn status(data_root: &Path, registry: &Registry) -> Vec<RangeStatus> {
    let now = Utc::now();
    registry
        .operators()
        .iter()
        .filter(|o| !o.range_urls.is_empty())
        .map(|o| {
            let file = read_file(data_root, &o.key);
            RangeStatus {
                operator: o.key.clone(),
                name: o.name.clone(),
                sources: o.range_urls.len(),
                prefixes: file.as_ref().map_or(0, |f| {
                    f.prefixes.iter().filter(|p| p.retired_at.is_none()).count()
                }),
                retired: file.as_ref().map_or(0, |f| {
                    f.prefixes.iter().filter(|p| p.retired_at.is_some()).count()
                }),
                tracked_since: file.as_ref().map(|f| f.tracked_since),
                fetched_at: file.as_ref().map(|f| f.fetched_at.clone()),
                fetches: file.as_ref().map_or(0, |f| f.fetches.len()),
                stale: is_stale(file.as_ref(), now),
            }
        })
        .collect()
}

/// True when any operator's cache is missing or older than 24 hours.
pub fn any_stale(data_root: &Path, registry: &Registry) -> bool {
    status(data_root, registry).iter().any(|s| s.stale)
}

/// Refreshes the cache over HTTP when any list is missing or older than 24
/// hours. Returns `None` when everything was fresh.
pub fn refresh_if_stale(
    data_root: &Path,
    registry: &Registry,
) -> Option<Result<Vec<UpdateOutcome>>> {
    any_stale(data_root, registry).then(|| update_online(data_root, registry))
}

/// Used when reporting: fails with a clear message if nothing is cached.
pub fn require_cache(data_root: &Path, registry: &Registry) -> Result<()> {
    if status(data_root, registry)
        .iter()
        .all(|s| s.fetched_at.is_none())
    {
        return Err(Error::invalid(
            "no bot IP ranges are cached yet; run `logloupe ranges update` while online",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testing::TempDataDir;

    const REGISTRY: &str = r#"
        [[operator]]
        key = "acme"
        name = "Acme"
        min_prefixes = 2
        range_urls = ["https://acme.test/a.json", "https://acme.test/b.json"]
        bots = [{ key = "acmebot", name = "AcmeBot", kind = "search", ua_match = ["acmebot"] }]

        [[operator]]
        key = "norange"
        name = "No Range"
        bots = [{ key = "otherbot", name = "OtherBot", kind = "other", ua_match = ["otherbot"] }]
    "#;

    fn list(prefixes: &[&str]) -> String {
        let entries: Vec<String> = prefixes
            .iter()
            .map(|p| {
                let key = if p.contains(':') {
                    "ipv6Prefix"
                } else {
                    "ipv4Prefix"
                };
                format!("{{\"{key}\": \"{p}\"}}")
            })
            .collect();
        format!(
            "{{\"creationTime\": \"x\", \"prefixes\": [{}]}}",
            entries.join(",")
        )
    }

    fn at(date: &str) -> DateTime<Utc> {
        format!("{date}T12:00:00Z").parse().unwrap()
    }

    fn day(date: &str) -> i64 {
        day_number(date.parse().unwrap())
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn cidrs() {
        assert_eq!(
            parse_cidr("192.0.2.0/24"),
            Some((ip_number(ip("192.0.2.0")), ip_number(ip("192.0.2.255"))))
        );
        assert_eq!(
            parse_cidr("192.0.2.7/32"),
            Some((ip_number(ip("192.0.2.7")), ip_number(ip("192.0.2.7"))))
        );
        assert_eq!(parse_cidr("192.0.2.7"), parse_cidr("192.0.2.7/32"));
        let (start, end) = parse_cidr("2001:db8::/32").unwrap();
        assert!(
            start <= ip_number(ip("2001:db8:ffff::1")) && ip_number(ip("2001:db8:ffff::1")) <= end
        );
        assert!(ip_number(ip("2001:db9::1")) > end);
        assert_eq!(
            parse_cidr("0.0.0.0/0").map(|(a, b)| b - a),
            Some(u128::from(u32::MAX))
        );
        for bad in ["", "192.0.2.0/33", "nope/24", "2001:db8::/129"] {
            assert_eq!(parse_cidr(bad), None, "{bad}");
        }
        // IPv4-mapped IPv6 addresses land in the IPv4 range.
        assert_eq!(
            ip_number(ip("::ffff:192.0.2.7")),
            ip_number(ip("192.0.2.7"))
        );
    }

    #[test]
    fn update_verify_and_history() {
        let tmp = TempDataDir::new("ranges");
        let root = tmp.0.root();
        let registry = Registry::from_toml(REGISTRY).unwrap();

        // Nothing cached: everything is unverifiable.
        let ranges = Ranges::load(root, &registry);
        assert_eq!(
            ranges.assess(0, ip("192.0.2.5")).verdict(day("2026-11-15")),
            Verdict::Unverifiable
        );

        // First fetch on 15 Nov: two lists merged for one operator.
        let first = |url: &str| -> std::result::Result<String, String> {
            Ok(if url.ends_with("a.json") {
                list(&["192.0.2.0/24", "198.51.100.0/24"])
            } else {
                list(&["2001:db8::/32"])
            })
        };
        let outcomes = update(root, &registry, at("2026-11-15"), &first).unwrap();
        assert_eq!(outcomes.len(), 1, "operators without lists are skipped");
        assert!(outcomes[0].updated);
        assert_eq!(
            (outcomes[0].prefixes, outcomes[0].added, outcomes[0].retired),
            (3, 3, 0)
        );

        let ranges = Ranges::load(root, &registry);
        let verdict =
            |ranges: &Ranges, addr: &str, date: &str| ranges.assess(0, ip(addr)).verdict(day(date));
        assert_eq!(
            verdict(&ranges, "192.0.2.5", "2026-11-15"),
            Verdict::Verified
        );
        assert_eq!(
            verdict(&ranges, "2001:db8:1::9", "2026-11-15"),
            Verdict::Verified
        );
        assert_eq!(
            verdict(&ranges, "::ffff:192.0.2.5", "2026-11-15"),
            Verdict::Verified
        );
        assert_eq!(verdict(&ranges, "203.0.113.5", "2026-11-15"), Verdict::Fake);
        // Within the 30-day grace before the first fetch: still judged.
        assert_eq!(verdict(&ranges, "203.0.113.5", "2026-10-20"), Verdict::Fake);
        // Long before the first fetch: nobody knows what the ranges were.
        assert_eq!(
            verdict(&ranges, "203.0.113.5", "2026-06-01"),
            Verdict::Unverifiable
        );
        // The operator with no lists stays unverifiable.
        assert_eq!(
            ranges
                .assess(1, ip("203.0.113.5"))
                .verdict(day("2026-11-15")),
            Verdict::Unverifiable
        );

        // 20 Nov: 198.51.100.0/24 is dropped and a new prefix appears.
        let second = |url: &str| -> std::result::Result<String, String> {
            Ok(if url.ends_with("a.json") {
                list(&["192.0.2.0/24", "203.0.113.0/24"])
            } else {
                list(&["2001:db8::/32"])
            })
        };
        let outcomes = update(root, &registry, at("2026-11-20"), &second).unwrap();
        assert_eq!(
            (outcomes[0].prefixes, outcomes[0].added, outcomes[0].retired),
            (3, 1, 1)
        );
        let ranges = Ranges::load(root, &registry);
        // The retired prefix still verifies requests from before it was dropped.
        assert_eq!(
            verdict(&ranges, "198.51.100.9", "2026-11-18"),
            Verdict::Verified
        );
        assert_eq!(
            verdict(&ranges, "198.51.100.9", "2026-11-19"),
            Verdict::Verified
        );
        assert_eq!(
            verdict(&ranges, "198.51.100.9", "2026-11-20"),
            Verdict::Fake
        );
        assert_eq!(
            verdict(&ranges, "203.0.113.5", "2026-11-21"),
            Verdict::Verified
        );

        let s = &status(root, &registry)[0];
        assert_eq!((s.prefixes, s.retired, s.fetches), (3, 1, 2));
        assert_eq!(s.tracked_since, Some("2026-11-15".parse().unwrap()));
    }

    #[test]
    fn whois_answers() {
        let answer = "route:          31.13.24.0/21\ndescr:          Facebook, Inc.\norigin:         AS32934\n\n\
                      route6:         2a03:2880::/32\norigin:         AS32934\nroute:      157.240.0.0/17\n";
        assert_eq!(
            parse_list("whois://whois.radb.net/-i origin AS32934", answer).unwrap(),
            vec!["31.13.24.0/21", "2a03:2880::/32", "157.240.0.0/17"]
        );
        assert!(parse_list("whois://x/y", "%  No entries found\n").is_err());
        assert!(parse_list("whois://x/y", "route: not-a-prefix\n").is_err());
        // Prefixes that would vouch for a large part of the internet are refused.
        for broad in ["0.0.0.0/0", "8.0.0.0/7", "::/0", "2000::/3"] {
            assert!(
                parse_list("whois://x/y", &format!("route: {broad}\n")).is_err(),
                "{broad}"
            );
            let json = format!("{{\"prefixes\": [{{\"ipv4Prefix\": \"{broad}\"}}]}}");
            assert!(parse_list("https://x/y.json", &json).is_err(), "{broad}");
        }
        assert!(parse_list("whois://x/y", "route: 17.0.0.0/8\nroute6: 2a03:2880::/29\n").is_ok());
        // A JSON list is still required for ordinary sources.
        assert!(parse_list("https://x/y.json", answer).is_err());
    }

    #[test]
    fn extra_ranges_confirm_but_never_accuse() {
        let tmp = TempDataDir::new("ranges-extra");
        let root = tmp.0.root();
        let registry = Registry::from_toml(&REGISTRY.replace(
            "min_prefixes = 2",
            "min_prefixes = 2
        extra_ranges = [\"203.0.113.0/24\"]",
        ))
        .unwrap();
        let today = day("2026-11-15");

        // Before any fetch: the extra range verifies, nothing is fake.
        let ranges = Ranges::load(root, &registry);
        assert_eq!(
            ranges.assess(0, ip("203.0.113.9")).verdict(today),
            Verdict::Verified
        );
        assert_eq!(
            ranges.assess(0, ip("192.0.2.9")).verdict(today),
            Verdict::Unverifiable
        );

        // After a fetch: published and extra ranges both verify.
        let fetch = |_: &str| -> std::result::Result<String, String> {
            Ok(list(&["192.0.2.0/24", "198.51.100.0/24"]))
        };
        update(root, &registry, at("2026-11-15"), &fetch).unwrap();
        let ranges = Ranges::load(root, &registry);
        assert_eq!(
            ranges.assess(0, ip("203.0.113.9")).verdict(today),
            Verdict::Verified
        );
        assert_eq!(
            ranges.assess(0, ip("192.0.2.9")).verdict(today),
            Verdict::Verified
        );
        assert_eq!(
            ranges.assess(0, ip("8.8.8.8")).verdict(today),
            Verdict::Fake
        );
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn suspicious_updates_are_refused() {
        let tmp = TempDataDir::new("ranges-refuse");
        let root = tmp.0.root();
        let registry = Registry::from_toml(REGISTRY).unwrap();
        let good = |_: &str| -> std::result::Result<String, String> {
            Ok(list(&["192.0.2.0/24", "198.51.100.0/24"]))
        };
        update(root, &registry, at("2026-11-15"), &good).unwrap();
        let before = std::fs::read_to_string(range_path(root, "acme")).unwrap();

        let cases: Vec<(
            &str,
            Box<dyn Fn(&str) -> std::result::Result<String, String>>,
        )> = vec![
            (
                "one list fails",
                Box::new(|url: &str| {
                    if url.ends_with("b.json") {
                        Err("timeout".into())
                    } else {
                        Ok(list(&["192.0.2.0/24", "198.51.100.0/24"]))
                    }
                }),
            ),
            (
                "too few prefixes",
                Box::new(|_: &str| Ok(list(&["192.0.2.0/24"]))),
            ),
            (
                "not JSON",
                Box::new(|_: &str| Ok("<html>blocked</html>".into())),
            ),
            (
                "wrong shape",
                Box::new(|_: &str| Ok("{\"ranges\": []}".into())),
            ),
            ("empty list", Box::new(|_: &str| Ok(list(&[])))),
            (
                "bad prefix",
                Box::new(|_: &str| Ok(list(&["192.0.2.0/24", "999.1.1.1/24"]))),
            ),
        ];
        for (name, fetch) in &cases {
            let outcomes = update(root, &registry, at("2026-11-16"), fetch.as_ref()).unwrap();
            assert!(!outcomes[0].updated, "{name}");
            assert!(outcomes[0].message.is_some(), "{name}");
            assert_eq!(outcomes[0].prefixes, 2, "{name}");
            assert_eq!(
                std::fs::read_to_string(range_path(root, "acme")).unwrap(),
                before,
                "{name}"
            );
        }
    }
}
