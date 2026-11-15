//! Storage upkeep: stats, the list of ingested files, pruning, clearing,
//! compaction and time zone changes.

use std::collections::BTreeSet;

use chrono::{DateTime, NaiveDate, Offset, TimeZone};
use chrono_tz::Tz;
use duckdb::params;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::ingest::{clear_data, rebuild_rollups};
use crate::store::{DataDir, Mode, OpenOptions, Store};

pub use crate::ingest::prune_before;

const ISO: &str = "'%Y-%m-%dT%H:%M:%SZ'";

#[derive(Debug, Clone, Serialize)]
pub struct SiteStats {
    pub slug: String,
    pub hosts: Vec<String>,
    pub timezone: String,
    pub retention: String,
    pub overlap: String,
    pub client_ip: String,
    pub edge_cache: bool,
    /// Database plus write-ahead log, in bytes.
    pub size_bytes: u64,
    pub rows: u64,
    /// UTC, ISO 8601.
    pub first_ts: Option<String>,
    pub last_ts: Option<String>,
    pub files: u64,
    pub user_agents: u64,
    /// Days that only exist as rollups.
    pub rollup_only_days: u64,
    pub pruned_before: Option<String>,
}

pub fn stats(dir: &DataDir, store: &Store) -> Result<SiteStats> {
    let settings = store.settings()?;
    let pruned_before = store.meta("pruned_before")?;
    let (rows, first_ts, last_ts, files, user_agents) = store.conn.query_row(
        &format!(
            "SELECT (SELECT count(*) FROM requests),
                    (SELECT strftime(min(ts), {ISO}) FROM requests),
                    (SELECT strftime(max(ts), {ISO}) FROM requests),
                    (SELECT count(*) FROM ingested_files),
                    (SELECT count(*) FROM user_agents)"
        ),
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    let rollup_only_days = match &pruned_before {
        Some(cutoff) => store.conn.query_row(
            &format!("SELECT count(*) FROM daily_totals WHERE day < DATE '{cutoff}'"),
            [],
            |row| row.get(0),
        )?,
        None => 0,
    };
    Ok(SiteStats {
        slug: store.slug().to_owned(),
        hosts: settings.hosts,
        timezone: settings.timezone.name().to_owned(),
        retention: settings.retention.to_string(),
        overlap: settings.overlap.to_string(),
        client_ip: settings.client_ip.to_string(),
        edge_cache: settings.edge_cache,
        size_bytes: dir.site_size(store.slug()),
        rows,
        first_ts,
        last_ts,
        files,
        user_agents,
        rollup_only_days,
        pruned_before,
    })
}

/// One site in a listing. `stats` is missing when another process has the
/// site open.
#[derive(Debug, Clone, Serialize)]
pub struct SiteEntry {
    pub slug: String,
    pub locked: bool,
    pub size_bytes: u64,
    pub stats: Option<SiteStats>,
}

/// Every site with its stats.
pub fn all_stats(dir: &DataDir, opts: &OpenOptions) -> Result<Vec<SiteEntry>> {
    let mut all = Vec::new();
    for slug in dir.list_sites()? {
        let entry = match Store::open(dir, &slug, Mode::ReadOnly, opts) {
            Ok(store) => SiteEntry {
                size_bytes: dir.site_size(&slug),
                stats: Some(stats(dir, &store)?),
                locked: false,
                slug,
            },
            Err(Error::SiteLocked(_)) => SiteEntry {
                size_bytes: dir.site_size(&slug),
                stats: None,
                locked: true,
                slug,
            },
            Err(e) => return Err(e),
        };
        all.push(entry);
    }
    Ok(all)
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct SettingsChange {
    /// The hostnames changed: rows already ingested keep their old source types.
    pub hosts_changed: bool,
    /// The time zone changed, and this many days exist only as rollups and
    /// keep their old day boundaries.
    pub stale_rollup_days: u64,
}

/// Applies a settings patch to an open site. A time zone change recomputes
/// local times and rebuilds the rollups.
pub fn update_settings(
    store: &Store,
    patch: &crate::store::SettingsPatch,
) -> Result<SettingsChange> {
    let before = store.settings()?;
    let mut settings = before.clone();
    patch.apply(&mut settings)?;
    let stale_rollup_days = if settings.timezone != before.timezone {
        change_timezone(store, settings.timezone)?
    } else {
        0
    };
    store.set_settings(&settings)?;
    Ok(SettingsChange {
        hosts_changed: settings.hosts != before.hosts,
        stale_rollup_days,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct IngestedFile {
    pub file_id: i32,
    pub source: String,
    pub path: String,
    pub size_bytes: u64,
    pub format: String,
    pub first_ts: Option<String>,
    pub last_ts: Option<String>,
    pub rows_ok: u64,
    pub rows_bad: u64,
    pub rows_skipped: u64,
    pub ingested_at: Option<String>,
}

pub fn files(store: &Store) -> Result<Vec<IngestedFile>> {
    let mut stmt = store.conn.prepare(&format!(
        "SELECT file_id, source, path, size_bytes, format, strftime(first_ts, {ISO}),
                strftime(last_ts, {ISO}), rows_ok, rows_bad, rows_skipped, strftime(ingested_at, {ISO})
         FROM ingested_files ORDER BY file_id"
    ))?;
    let rows = stmt.query_map([], |row| {
        Ok(IngestedFile {
            file_id: row.get(0)?,
            source: row.get(1)?,
            path: row.get(2)?,
            size_bytes: row.get(3)?,
            format: row.get(4)?,
            first_ts: row.get(5)?,
            last_ts: row.get(6)?,
            rows_ok: row.get(7)?,
            rows_bad: row.get(8)?,
            rows_skipped: row.get(9)?,
            ingested_at: row.get(10)?,
        })
    })?;
    Ok(rows.collect::<duckdb::Result<_>>()?)
}

fn pruned_before(store: &Store) -> Result<Option<NaiveDate>> {
    Ok(store.meta("pruned_before")?.and_then(|d| d.parse().ok()))
}

/// Local days (as days since the Unix epoch) that have raw rows matching `condition`.
fn days_where(store: &Store, condition: &str) -> Result<BTreeSet<i64>> {
    let mut stmt = store.conn.prepare(&format!(
        "SELECT DISTINCT datediff('day', DATE '1970-01-01', CAST(ts_local AS DATE)) FROM requests WHERE {condition}"
    ))?;
    let days = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    Ok(days.collect::<duckdb::Result<_>>()?)
}

/// Deletes one ingested file's rows and rebuilds the rollups for its days.
/// Returns the number of rows deleted.
pub fn delete_file(store: &Store, file_id: i32) -> Result<u64> {
    let conn = &store.conn;
    let known: i64 = conn.query_row(
        "SELECT count(*) FROM ingested_files WHERE file_id = ?",
        params![file_id],
        |row| row.get(0),
    )?;
    if known == 0 {
        return Err(Error::invalid(format!(
            "no ingested file with id {file_id}"
        )));
    }
    let days = days_where(store, &format!("file_id = {file_id}"))?;
    let cutoff = pruned_before(store)?;
    conn.execute_batch("BEGIN TRANSACTION")?;
    let result = (|| -> Result<u64> {
        let deleted = conn.execute("DELETE FROM requests WHERE file_id = ?", params![file_id])?;
        conn.execute(
            "DELETE FROM ingested_files WHERE file_id = ?",
            params![file_id],
        )?;
        rebuild_rollups(conn, &days, cutoff)?;
        Ok(deleted as u64)
    })();
    conn.execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
    result
}

/// Deletes raw rows before `before`. With `rollups`, deletes those days'
/// rollups as well. Returns the number of raw rows deleted.
pub fn prune(store: &Store, before: NaiveDate, rollups: bool) -> Result<u64> {
    let deleted = prune_before(store, before)?;
    if rollups {
        store.conn.execute_batch(&format!(
            "DELETE FROM daily_totals WHERE day < DATE '{before}';
             DELETE FROM daily_bots WHERE day < DATE '{before}';
             DELETE FROM daily_sources WHERE day < DATE '{before}';"
        ))?;
    }
    Ok(deleted)
}

/// Deletes all raw rows and rollups but keeps the site and its settings.
pub fn clear(store: &Store) -> Result<()> {
    clear_data(&store.conn)
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct CompactResult {
    pub bytes_before: u64,
    pub bytes_after: u64,
}

/// Rewrites the database into a new file and swaps it in. Deletes alone do
/// not reliably shrink a DuckDB file.
pub fn compact(dir: &DataDir, slug: &str, opts: &OpenOptions) -> Result<CompactResult> {
    if !dir.site_exists(slug) {
        return Err(Error::SiteNotFound(slug.to_owned()));
    }
    let path = dir.site_path(slug);
    let mut new_name = path.as_os_str().to_owned();
    new_name.push(".compact");
    let new_path = std::path::PathBuf::from(new_name);
    let mut old_name = path.as_os_str().to_owned();
    old_name.push(".old");
    let old_path = std::path::PathBuf::from(old_name);
    let _ = std::fs::remove_file(&new_path);

    let bytes_before = dir.site_size(slug);
    {
        let store = Store::open(dir, slug, Mode::ReadWrite, opts)?;
        let current: String = store
            .conn
            .query_row("SELECT current_database()", [], |row| row.get(0))?;
        let copied = store.conn.execute_batch(&format!(
            "CHECKPOINT;
             ATTACH {} AS loga_compacted;
             COPY FROM DATABASE \"{}\" TO loga_compacted;
             DETACH loga_compacted;",
            crate::report::quote(&new_path.to_string_lossy()),
            current.replace('"', "\"\""),
        ));
        if let Err(e) = copied {
            drop(store);
            let _ = std::fs::remove_file(&new_path);
            return Err(e.into());
        }
    }

    std::fs::rename(&path, &old_path)?;
    if let Err(e) = std::fs::rename(&new_path, &path) {
        // Put the original back rather than leave the site missing.
        let _ = std::fs::rename(&old_path, &path);
        return Err(e.into());
    }
    let _ = std::fs::remove_file(&old_path);
    for leftover in [".wal", ".compact.wal"] {
        let mut name = path.as_os_str().to_owned();
        name.push(leftover);
        let _ = std::fs::remove_file(name);
    }
    Ok(CompactResult {
        bytes_before,
        bytes_after: dir.site_size(slug),
    })
}

/// Changes the site's time zone: recomputes `ts_local` for every raw row and
/// rebuilds the rollups. Days that only exist as rollups keep their old
/// boundaries; the number of such days is returned.
pub fn change_timezone(store: &Store, timezone: Tz) -> Result<u64> {
    let conn = &store.conn;
    let cutoff = pruned_before(store)?;
    let stale_days: u64 = match cutoff {
        Some(cutoff) => conn.query_row(
            &format!("SELECT count(*) FROM daily_totals WHERE day < DATE '{cutoff}'"),
            [],
            |row| row.get(0),
        )?,
        None => 0,
    };
    let (first, last): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT epoch(min(ts))::BIGINT, epoch(max(ts))::BIGINT FROM requests",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    conn.execute_batch("BEGIN TRANSACTION")?;
    let result = (|| -> Result<()> {
        let old_days = days_where(store, "true")?;
        if let (Some(first), Some(last)) = (first, last) {
            // Offsets only change on 15-minute boundaries, so walk the data's
            // span in 15-minute steps and update each run of equal offsets at once.
            let offset_at = |ts: i64| -> i64 {
                DateTime::from_timestamp(ts, 0)
                    .map(|utc| {
                        i64::from(
                            timezone
                                .offset_from_utc_datetime(&utc.naive_utc())
                                .fix()
                                .local_minus_utc(),
                        )
                    })
                    .unwrap_or(0)
            };
            let mut start = first.div_euclid(900) * 900;
            let mut offset = offset_at(start);
            let mut slot = start;
            loop {
                slot += 900;
                let next = if slot > last {
                    None
                } else {
                    Some(offset_at(slot))
                };
                if next != Some(offset) {
                    conn.execute_batch(&format!(
                        "UPDATE requests SET ts_local = ts + to_seconds({offset})
                         WHERE ts >= make_timestamp({}) AND ts < make_timestamp({})",
                        start * 1_000_000,
                        slot * 1_000_000
                    ))?;
                    match next {
                        Some(next) => {
                            offset = next;
                            start = slot;
                        }
                        None => break,
                    }
                }
            }
        }
        let mut days = days_where(store, "true")?;
        days.extend(old_days);
        rebuild_rollups(conn, &days, cutoff)?;
        store.set_meta("timezone", timezone.name())?;
        Ok(())
    })();
    conn.execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
    result.map(|()| stale_days)
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ReenrichResult {
    /// User agents whose category, family or bot changed.
    pub user_agents_changed: u64,
    /// Distinct address and bot pairs checked against the ranges.
    pub addresses_checked: u64,
    pub verified: u64,
    pub fake: u64,
    pub unverifiable: u64,
    /// Distinct addresses looked up for country and network; 0 when no geo
    /// database is installed.
    pub addresses_located: u64,
}

/// Re-runs user-agent classification and bot verification over the stored
/// rows with the current registry and range cache, then rebuilds the rollups.
/// Used after the registry or the ranges change, and for rows ingested before
/// verification existed.
pub fn reenrich(store: &Store) -> Result<ReenrichResult> {
    use crate::bots::{self, Ranges, Registry};
    use std::collections::HashMap;
    use std::net::IpAddr;

    let conn = &store.conn;
    let registry = Registry::load(store.data_root())?;
    let ranges = Ranges::load(store.data_root(), &registry);
    let cutoff = pruned_before(store)?;

    conn.execute_batch("BEGIN TRANSACTION")?;
    let result = (|| -> Result<ReenrichResult> {
        // 1. User agents.
        let mut operators: HashMap<u32, usize> = HashMap::new();
        let mut changed = 0u64;
        conn.execute_batch(
            "CREATE OR REPLACE TEMP TABLE loga_ua (ua_id UINTEGER, category VARCHAR, family VARCHAR, bot_key VARCHAR, is_bot BOOLEAN)",
        )?;
        {
            let mut stmt =
                conn.prepare("SELECT ua_id, ua, category, family, bot_key FROM user_agents")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?;
            let mut appender = conn.appender("loga_ua")?;
            for row in rows {
                let (ua_id, ua, category, family, bot_key) = row?;
                let (class, bot) = bots::classify(&registry, &ua);
                let new_key = bot.map(|id| registry.bot(id).key.clone());
                if let Some(id) = bot {
                    operators.insert(ua_id, id.operator);
                }
                if class.category.as_str() != category
                    || class.family != family
                    || new_key != bot_key
                {
                    changed += 1;
                }
                appender.append_row(params![
                    ua_id,
                    class.category.as_str(),
                    class.family.as_deref(),
                    new_key.as_deref(),
                    bot.is_some()
                ])?;
            }
            appender.flush()?;
        }
        conn.execute_batch(
            "UPDATE user_agents SET category = t.category, family = t.family, bot_key = t.bot_key
             FROM loga_ua t WHERE user_agents.ua_id = t.ua_id;
             -- A user agent now known not to be a browser cannot have page views.
             UPDATE requests SET is_page = false
             WHERE is_page AND ua_id IN (SELECT ua_id FROM loga_ua WHERE category <> 'browser');
             UPDATE requests SET bot_verify = NULL
             WHERE bot_verify IS NOT NULL AND ua_id NOT IN (SELECT ua_id FROM loga_ua WHERE is_bot);",
        )?;

        // 2. Verification, once per distinct address and bot user agent.
        conn.execute_batch(
            "CREATE OR REPLACE TEMP TABLE loga_verify
             (ip VARCHAR, ua_id UINTEGER, current BOOLEAN, verified_until BIGINT, fake_from BIGINT)",
        )?;
        let mut checked = 0u64;
        {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT ip, ua_id FROM requests WHERE ua_id IN (SELECT ua_id FROM loga_ua WHERE is_bot)",
            )?;
            let pairs = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
            })?;
            let mut appender = conn.appender("loga_verify")?;
            for pair in pairs {
                let (ip, ua_id) = pair?;
                let (Ok(addr), Some(operator)) = (ip.parse::<IpAddr>(), operators.get(&ua_id))
                else {
                    continue;
                };
                let a = ranges.assess(*operator, addr);
                appender.append_row(params![
                    ip,
                    ua_id,
                    a.current,
                    a.verified_until,
                    a.fake_from
                ])?;
                checked += 1;
            }
            appender.flush()?;
        }
        conn.execute_batch(
            "UPDATE requests SET bot_verify = CASE
                 WHEN t.current THEN 'verified'
                 WHEN t.verified_until IS NOT NULL
                      AND datediff('day', DATE '1970-01-01', CAST(requests.ts AS DATE)) <= t.verified_until THEN 'verified'
                 WHEN t.fake_from IS NOT NULL
                      AND datediff('day', DATE '1970-01-01', CAST(requests.ts AS DATE)) >= t.fake_from THEN 'fake'
                 ELSE 'unverifiable' END
             FROM loga_verify t WHERE requests.ip = t.ip AND requests.ua_id = t.ua_id;",
        )?;

        // 3. Country and network, once per distinct address. Skipped when no
        // geo database is installed, so existing values are not wiped.
        let geo = crate::geo::GeoDb::load(store.data_root());
        let mut located = 0u64;
        if !geo.is_empty() {
            conn.execute_batch(
                "CREATE OR REPLACE TEMP TABLE loga_geo
                 (ip VARCHAR, country VARCHAR, asn UINTEGER, asn_org VARCHAR)",
            )?;
            {
                let mut stmt = conn.prepare("SELECT DISTINCT ip FROM requests")?;
                let addresses = stmt.query_map([], |row| row.get::<_, String>(0))?;
                let mut appender = conn.appender("loga_geo")?;
                for address in addresses {
                    let ip = address?;
                    let Ok(addr) = ip.parse::<IpAddr>() else {
                        continue;
                    };
                    let place = geo.lookup(addr);
                    appender.append_row(params![
                        ip,
                        place.country.as_deref(),
                        place.asn,
                        place.asn_org.as_deref()
                    ])?;
                    located += 1;
                }
                appender.flush()?;
            }
            conn.execute_batch(
                "UPDATE requests SET country = t.country, asn = t.asn, asn_org = t.asn_org
                 FROM loga_geo t WHERE requests.ip = t.ip;",
            )?;
        }

        // 4. Rollups.
        let days = days_where(store, "true")?;
        rebuild_rollups(conn, &days, cutoff)?;

        let count = |verdict: &str| -> Result<u64> {
            Ok(conn.query_row(
                "SELECT count(*) FROM requests WHERE bot_verify = ?",
                params![verdict],
                |row| row.get(0),
            )?)
        };
        Ok(ReenrichResult {
            user_agents_changed: changed,
            addresses_checked: checked,
            verified: count("verified")?,
            fake: count("fake")?,
            unverifiable: count("unverifiable")?,
            addresses_located: located,
        })
    })();
    conn.execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
    result
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::ingest::{IngestOptions, ingest};
    use crate::store::SiteSettings;
    use crate::store::testing::TempDataDir;

    fn line(day: u32, hour: u32, path: &str) -> String {
        format!(
            "203.0.113.1 - - [{day:02}/Nov/2026:{hour:02}:30:00 +0000] \"GET {path} HTTP/2.0\" 200 100 \"-\" \"Mozilla/5.0 Chrome/1.0 Safari/1.0\"\n"
        )
    }

    fn site(name: &str, logs: &[(&str, String)]) -> (TempDataDir, Store) {
        let tmp = TempDataDir::new(name);
        let mut store = Store::create(
            &tmp.0,
            "my-site.com",
            &SiteSettings::default(),
            &OpenOptions::default(),
        )
        .unwrap();
        for (file, content) in logs {
            let path = tmp.0.root().join(file);
            std::fs::write(&path, content).unwrap();
            ingest(
                &mut store,
                &[path],
                &IngestOptions::default(),
                &mut |_| {},
                &AtomicBool::new(false),
            )
            .unwrap();
        }
        (tmp, store)
    }

    fn count(store: &Store, sql: &str) -> i64 {
        store.conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn stats_files_and_delete_file() {
        let (tmp, store) = site(
            "maintain-files",
            &[
                (
                    "a.log",
                    format!("{}{}", line(14, 10, "/a/"), line(14, 11, "/b/")),
                ),
                ("b.log", line(15, 10, "/c/")),
            ],
        );
        let s = stats(&tmp.0, &store).unwrap();
        assert_eq!((s.rows, s.files, s.user_agents), (3, 2, 1));
        assert_eq!(s.first_ts.as_deref(), Some("2026-11-14T10:30:00Z"));
        assert_eq!(s.last_ts.as_deref(), Some("2026-11-15T10:30:00Z"));
        assert!(s.size_bytes > 0);

        let listed = files(&store).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed[0].path.ends_with("a.log"));
        assert_eq!(listed[0].rows_ok, 2);

        assert_eq!(delete_file(&store, listed[0].file_id).unwrap(), 2);
        assert_eq!(count(&store, "SELECT count(*) FROM requests"), 1);
        assert_eq!(count(&store, "SELECT count(*) FROM daily_totals"), 1);
        assert!(delete_file(&store, 99).is_err());
    }

    #[test]
    fn prune_keeps_rollups_unless_asked() {
        let (_tmp, store) = site(
            "maintain-prune",
            &[(
                "a.log",
                format!("{}{}", line(14, 10, "/a/"), line(15, 10, "/b/")),
            )],
        );
        let cutoff = NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        assert_eq!(prune(&store, cutoff, false).unwrap(), 1);
        assert_eq!(count(&store, "SELECT count(*) FROM daily_totals"), 2);
        assert_eq!(prune(&store, cutoff, true).unwrap(), 0);
        assert_eq!(count(&store, "SELECT count(*) FROM daily_totals"), 1);
        clear(&store).unwrap();
        assert_eq!(count(&store, "SELECT count(*) FROM daily_totals"), 0);
        assert_eq!(store.meta("pruned_before").unwrap(), None);
    }

    #[test]
    fn compact_keeps_data_and_settings() {
        let content: String = (0..24).map(|h| line(14, h, "/a/")).collect();
        let (tmp, store) = site("maintain-compact", &[("a.log", content)]);
        drop(store);
        let opts = OpenOptions::default();
        let result = compact(&tmp.0, "my-site.com", &opts).unwrap();
        assert!(result.bytes_after > 0);
        let store = Store::open(&tmp.0, "my-site.com", Mode::ReadOnly, &opts).unwrap();
        assert_eq!(count(&store, "SELECT count(*) FROM requests"), 24);
        assert_eq!(count(&store, "SELECT requests FROM daily_totals"), 24);
        assert_eq!(store.settings().unwrap(), SiteSettings::default());
    }

    #[test]
    fn changing_time_zone_moves_day_boundaries() {
        // 23:30 UTC on the 14th is 10:30 on the 15th in Sydney.
        let (_tmp, store) = site(
            "maintain-tz",
            &[(
                "a.log",
                format!("{}{}", line(14, 10, "/a/"), line(14, 23, "/b/")),
            )],
        );
        assert_eq!(
            count(
                &store,
                "SELECT requests FROM daily_totals WHERE day = DATE '2026-11-14'"
            ),
            2
        );
        assert_eq!(
            change_timezone(&store, chrono_tz::Australia::Sydney).unwrap(),
            0
        );
        assert_eq!(
            count(
                &store,
                "SELECT requests FROM daily_totals WHERE day = DATE '2026-11-14'"
            ),
            1
        );
        assert_eq!(
            count(
                &store,
                "SELECT requests FROM daily_totals WHERE day = DATE '2026-11-15'"
            ),
            1
        );
        assert_eq!(
            count(
                &store,
                "SELECT count(*) FROM requests WHERE ts_local = ts + INTERVAL 11 HOUR"
            ),
            2
        );
        assert_eq!(
            store.settings().unwrap().timezone,
            chrono_tz::Australia::Sydney
        );
    }
}
