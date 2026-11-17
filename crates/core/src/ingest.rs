//! The ingest pipeline: source → lines → parse → enrich → append → rollups.
//!
//! Streaming throughout. Each file is one transaction, so a failed or
//! cancelled ingest leaves no partial rows.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, Read};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Days, NaiveDate, Offset, TimeZone};
use chrono_tz::Tz;
use duckdb::types::{TimeUnit, Value};
use duckdb::{Connection, params};
use serde::Serialize;

use crate::bots::{self, Assessment, Ranges, Registry};
use crate::classify::SourceRules;
use crate::classify::source::{DEFAULT_RULES, referer_host};
use crate::classify::ua::{UaCategory, UaClass};
use crate::error::{Error, Result};
use crate::format::detect;
use crate::format::{LogFormat, Request};
use crate::geo::{GeoDb, Location};
use crate::source;
use crate::store::{Mode, OverlapPolicy, Retention, Store};

/// Size of the head and tail windows used to fingerprint a file (spec 4.3).
const FINGERPRINT_WINDOW: usize = 64 * 1024;
/// Decompressed bytes read up front for de-duplication and format detection.
const PROBE_BYTES: usize = 256 * 1024;
const MAX_BAD_SAMPLES: usize = 20;
/// Abort when more than 20% of the first 1,000 lines fail to parse.
const WRONG_FORMAT_WINDOW: u64 = 1_000;
const PROGRESS_EVERY_LINES: u64 = 20_000;
const IP_CACHE_LIMIT: usize = 500_000;

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct IngestOptions {
    /// A format name, or `None` to auto-detect.
    pub format: Option<String>,
    /// Re-ingest a file that was already ingested, replacing its rows.
    pub force: bool,
    /// Overrides the site's overlap policy for this run.
    pub overlap: Option<OverlapPolicy>,
    /// Run reverse DNS checks afterwards for bots that IP ranges cannot settle.
    pub dns: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Progress<'a> {
    pub path: &'a Path,
    /// Bytes read from disk (compressed bytes for gzip).
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub rows: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    /// A file not seen before.
    Ingested,
    /// A file seen before that has grown; only the new lines were ingested.
    Resumed,
    /// Already ingested and nothing new: a no-op.
    Unchanged,
    /// The file has no complete lines.
    Empty,
}

#[derive(Debug, Clone, Serialize)]
pub struct BadLine {
    pub reason: String,
    /// The start of the line, at most 300 characters.
    pub line: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileReport {
    pub path: PathBuf,
    pub status: FileStatus,
    pub file_id: Option<i32>,
    pub format: Option<String>,
    pub gzip: bool,
    /// Size on disk (the compressed size for gzip).
    pub size_bytes: u64,
    pub rows_ok: u64,
    pub rows_bad: u64,
    pub rows_skipped: u64,
    /// Unix seconds, UTC, of the rows added by this run.
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub bad_samples: Vec<BadLine>,
    pub seconds: f64,
}

/// Ingests files in order, one transaction each. Stops at the first error;
/// files before it stay committed.
pub fn ingest(
    store: &mut Store,
    paths: &[PathBuf],
    opts: &IngestOptions,
    progress: &mut dyn FnMut(Progress<'_>),
    cancel: &AtomicBool,
) -> Result<Vec<FileReport>> {
    let mut session = Session::begin(store, opts)?;
    let mut reports = Vec::with_capacity(paths.len());
    for path in paths {
        reports.push(session.file(store, path, None, opts, progress, cancel)?);
    }
    session.finish(store, opts)?;
    Ok(reports)
}

/// Where a file came from when it is not the local path it is read from
/// (spec 9). Recorded in `ingested_files` and used in reports and errors.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Origin<'a> {
    /// `ssh:<host>`.
    pub source: &'a str,
    /// The path on the server.
    pub path: &'a str,
}

/// One ingest run: the set-up shared by its files, and the steps that run
/// once after the last of them.
pub(crate) struct Session {
    ctx: Context,
    retention: Retention,
    any_rows: bool,
}

impl Session {
    /// Loads the enrichment data. With retention `none`, clears the site.
    pub(crate) fn begin(store: &mut Store, opts: &IngestOptions) -> Result<Session> {
        if store.mode() != Mode::ReadWrite {
            return Err(Error::invalid("ingest needs the site opened read-write"));
        }
        let settings = store.settings()?;
        if settings.retention == Retention::None {
            clear_data(&store.conn)?;
        }

        let rules = SourceRules::from_toml(DEFAULT_RULES, &settings.hosts)
            .map_err(|e| Error::invalid(format!("invalid source rules: {e}")))?;
        let registry = Registry::load(store.data_root())?;
        let ranges = Ranges::load(store.data_root(), &registry);
        Ok(Session {
            ctx: Context {
                rules,
                registry,
                ranges,
                geo: GeoDb::load(store.data_root()),
                timezone: settings.timezone,
                overlap: opts.overlap.unwrap_or(settings.overlap),
            },
            retention: settings.retention,
            any_rows: false,
        })
    }

    /// Ingests one file in its own transaction. `local` is where the bytes
    /// are read from; `origin` says where they came from if that is elsewhere.
    pub(crate) fn file(
        &mut self,
        store: &mut Store,
        local: &Path,
        origin: Option<Origin<'_>>,
        opts: &IngestOptions,
        progress: &mut dyn FnMut(Progress<'_>),
        cancel: &AtomicBool,
    ) -> Result<FileReport> {
        let report = ingest_file(store, local, origin, opts, &self.ctx, progress, cancel)?;
        self.any_rows |= report.rows_ok > 0;
        Ok(report)
    }

    /// Reverse DNS and retention pruning, once the files are in.
    pub(crate) fn finish(self, store: &mut Store, opts: &IngestOptions) -> Result<()> {
        if opts.dns && self.any_rows {
            // A DNS problem must not fail an ingest that has already succeeded.
            let _ = bots::rdns::apply(
                store,
                &self.ctx.registry,
                &bots::rdns::SystemResolver,
                chrono::Utc::now().date_naive(),
            );
        }

        if let Retention::Days(days) = self.retention {
            let today = chrono::Utc::now()
                .with_timezone(&self.ctx.timezone)
                .date_naive();
            if let Some(cutoff) = today.checked_sub_days(Days::new(u64::from(days))) {
                prune_before(store, cutoff)?;
            }
        }
        Ok(())
    }
}

/// Deletes raw rows dated before `cutoff` (a day in the site's time zone).
/// Rollups are kept. Returns the number of rows deleted.
pub fn prune_before(store: &Store, cutoff: NaiveDate) -> Result<u64> {
    let deleted = store.conn.execute(
        &format!("DELETE FROM requests WHERE ts_local < TIMESTAMP '{cutoff} 00:00:00'"),
        [],
    )?;
    // Rollups for days before this date can no longer be rebuilt from raw rows.
    let previous = store
        .meta("pruned_before")?
        .and_then(|d| d.parse::<NaiveDate>().ok());
    if previous.is_none_or(|p| cutoff > p) {
        store.set_meta("pruned_before", &cutoff.to_string())?;
    }
    Ok(deleted as u64)
}

/// Deletes all raw rows and rollups, keeping the site and its settings.
pub fn clear_data(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "BEGIN;
         DELETE FROM requests; DELETE FROM ingested_files; DELETE FROM user_agents;
         DELETE FROM daily_totals; DELETE FROM daily_bots; DELETE FROM daily_sources;
         DELETE FROM meta WHERE key = 'pruned_before';
         COMMIT;",
    )?;
    Ok(())
}

struct Context {
    registry: Registry,
    ranges: Ranges,
    geo: GeoDb,
    rules: SourceRules,
    timezone: Tz,
    overlap: OverlapPolicy,
}

/// A row of `ingested_files`.
#[derive(Debug, Clone)]
struct StoredFile {
    file_id: i32,
    path: String,
    bytes_consumed: u64,
    head_hash: String,
    tail_hash: String,
    format: String,
    first_ts: Option<i64>,
    last_ts: Option<i64>,
    rows_ok: u64,
    rows_bad: u64,
    rows_skipped: u64,
}

fn stored_files(conn: &Connection) -> Result<Vec<StoredFile>> {
    let mut stmt = conn.prepare(
        "SELECT file_id, path, bytes_consumed, head_hash, tail_hash, format,
                epoch(first_ts)::BIGINT, epoch(last_ts)::BIGINT, rows_ok, rows_bad, rows_skipped
         FROM ingested_files ORDER BY file_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(StoredFile {
            file_id: row.get(0)?,
            path: row.get(1)?,
            bytes_consumed: row.get(2)?,
            head_hash: row.get(3)?,
            tail_hash: row.get(4)?,
            format: row.get(5)?,
            first_ts: row.get(6)?,
            last_ts: row.get(7)?,
            rows_ok: row.get(8)?,
            rows_bad: row.get(9)?,
            rows_skipped: row.get(10)?,
        })
    })?;
    Ok(rows.collect::<duckdb::Result<_>>()?)
}

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Rolling fingerprint of the consumed part of a file: the first and the last
/// 64 KB of decompressed content.
struct Fingerprint {
    head: Vec<u8>,
    tail: Vec<u8>,
    total: u64,
}

impl Fingerprint {
    fn new() -> Self {
        Fingerprint {
            head: Vec::with_capacity(FINGERPRINT_WINDOW),
            tail: Vec::with_capacity(FINGERPRINT_WINDOW * 2),
            total: 0,
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        if self.head.len() < FINGERPRINT_WINDOW {
            let take = (FINGERPRINT_WINDOW - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..take]);
        }
        self.tail.extend_from_slice(bytes);
        if self.tail.len() > FINGERPRINT_WINDOW * 2 {
            let cut = self.tail.len() - FINGERPRINT_WINDOW;
            self.tail.drain(..cut);
        }
        self.total += bytes.len() as u64;
    }

    fn head_hash(&self) -> String {
        hash(&self.head)
    }

    fn tail_hash(&self) -> String {
        let start = self.tail.len().saturating_sub(FINGERPRINT_WINDOW);
        hash(&self.tail[start..])
    }
}

fn file_error(path: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |source| Error::File {
        path: path.to_path_buf(),
        source,
    }
}

/// Reads up to `PROBE_BYTES` of decompressed content from the start of a file.
/// Errors name `path`, which differs from `local` for a fetched file.
fn read_probe(local: &Path, path: &Path) -> Result<Vec<u8>> {
    let source = source::open(local).map_err(file_error(path))?;
    let mut probe = Vec::with_capacity(PROBE_BYTES);
    source
        .reader
        .take(PROBE_BYTES as u64)
        .read_to_end(&mut probe)
        .map_err(file_error(path))?;
    Ok(probe)
}

/// Finds stored files whose head matches the start of this file. A stored
/// file's head covers the first 64 KB, or all of it when it was shorter.
fn head_matches<'a>(probe: &[u8], stored: &'a [StoredFile]) -> Vec<&'a StoredFile> {
    let mut hashes: HashMap<usize, String> = HashMap::new();
    let mut matches: Vec<&StoredFile> = stored
        .iter()
        .filter(|f| {
            let len = (f.bytes_consumed as usize).min(FINGERPRINT_WINDOW);
            len > 0
                && probe.len() >= len
                && *hashes.entry(len).or_insert_with(|| hash(&probe[..len])) == f.head_hash
        })
        .collect();
    // Prefer the candidate that covers the most of the file.
    matches.sort_by_key(|f| std::cmp::Reverse(f.bytes_consumed));
    matches
}

/// Reads and discards `count` bytes, feeding them to the fingerprint.
/// Returns false when the file ends first.
fn skip_bytes(
    reader: &mut dyn BufRead,
    mut count: u64,
    fp: &mut Fingerprint,
) -> std::io::Result<bool> {
    while count > 0 {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(false);
        }
        let take = (available.len() as u64).min(count) as usize;
        fp.feed(&available[..take]);
        reader.consume(take);
        count -= take as u64;
    }
    Ok(true)
}

/// Time ranges already covered by other files, for the overlap policy.
struct Covered {
    /// Disjoint, sorted by start.
    merged: Vec<(i64, i64)>,
    files: Vec<(i64, i64, String)>,
}

impl Covered {
    fn new(stored: &[StoredFile], except: Option<i32>) -> Self {
        let files: Vec<(i64, i64, String)> = stored
            .iter()
            .filter(|f| Some(f.file_id) != except)
            .filter_map(|f| Some((f.first_ts?, f.last_ts?, f.path.clone())))
            .collect();
        let mut ranges: Vec<(i64, i64)> = files.iter().map(|(a, b, _)| (*a, *b)).collect();
        ranges.sort_unstable();
        let mut merged: Vec<(i64, i64)> = Vec::with_capacity(ranges.len());
        for (start, end) in ranges {
            match merged.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        Covered { merged, files }
    }

    fn contains(&self, ts: i64) -> bool {
        let after = self.merged.partition_point(|(start, _)| *start <= ts);
        after > 0 && ts <= self.merged[after - 1].1
    }

    fn files_containing(&self, ts: i64) -> Vec<String> {
        self.files
            .iter()
            .filter(|(start, end, _)| (*start..=*end).contains(&ts))
            .map(|(_, _, path)| path.clone())
            .collect()
    }
}

/// Distinct user agents: ids already in the database plus ones met in this run.
struct UaCache {
    /// id, category, and the operator whose ranges vouch for the claimed bot.
    known: HashMap<Box<str>, (u32, UaCategory, Option<usize>)>,
    next_id: u32,
    added: Vec<(u32, Box<str>, UaClass, Option<String>)>,
}

impl UaCache {
    fn load(conn: &Connection, registry: &Registry) -> Result<Self> {
        let mut stmt = conn.prepare("SELECT ua_id, ua, category, bot_key FROM user_agents")?;
        let mut known = HashMap::new();
        let mut next_id = 1;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            let (id, ua, category, bot_key) = row?;
            next_id = next_id.max(id + 1);
            let category = UaCategory::parse(&category).unwrap_or(UaCategory::Unknown);
            let operator = bot_key
                .and_then(|key| registry.find_key(&key))
                .map(|id| id.operator);
            known.insert(ua.into_boxed_str(), (id, category, operator));
        }
        Ok(UaCache {
            known,
            next_id,
            added: Vec::new(),
        })
    }

    /// `ua` is the raw string, or `""` when the log had none.
    fn get(&mut self, ua: &str, registry: &Registry) -> (u32, UaCategory, Option<usize>) {
        if let Some(found) = self.known.get(ua) {
            return *found;
        }
        let (class, bot) = bots::classify(registry, ua);
        let entry = (self.next_id, class.category, bot.map(|id| id.operator));
        self.next_id += 1;
        self.known.insert(ua.into(), entry);
        let bot_key = bot.map(|id| registry.bot(id).key.clone());
        self.added.push((entry.0, ua.into(), class, bot_key));
        entry
    }

    fn flush(&mut self, conn: &Connection) -> Result<()> {
        let mut appender = conn.appender("user_agents")?;
        for (id, ua, class, bot_key) in self.added.drain(..) {
            appender.append_row(params![
                id,
                &*ua,
                class.category.as_str(),
                class.family.as_deref(),
                bot_key.as_deref()
            ])?;
        }
        appender.flush()?;
        Ok(())
    }
}

/// UTC offset of the site's time zone, recomputed only when the timestamp
/// moves into a different 15-minute slot (offsets never change inside one).
struct LocalClock {
    timezone: Tz,
    slot: i64,
    offset: i64,
}

impl LocalClock {
    fn new(timezone: Tz) -> Self {
        LocalClock {
            timezone,
            slot: i64::MIN,
            offset: 0,
        }
    }

    fn offset(&mut self, ts: i64) -> i64 {
        let slot = ts.div_euclid(900);
        if slot != self.slot {
            self.slot = slot;
            self.offset = DateTime::from_timestamp(ts, 0)
                .map(|utc| {
                    i64::from(
                        self.timezone
                            .offset_from_utc_datetime(&utc.naive_utc())
                            .fix()
                            .local_minus_utc(),
                    )
                })
                .unwrap_or(0);
        }
        self.offset
    }
}

fn ip_net(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            format!("{a}.{b}.{c}.0/24")
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            let net = std::net::Ipv6Addr::new(s[0], s[1], s[2], 0, 0, 0, 0, 0);
            format!("{net}/48")
        }
    }
}

/// Rolls the transaction back unless it was committed.
struct Transaction<'a> {
    conn: &'a Connection,
    open: bool,
}

impl<'a> Transaction<'a> {
    fn begin(conn: &'a Connection) -> Result<Self> {
        conn.execute_batch("BEGIN TRANSACTION")?;
        Ok(Transaction { conn, open: true })
    }

    fn commit(mut self) -> Result<()> {
        self.open = false;
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if self.open {
            let _ = self.conn.execute_batch("ROLLBACK");
        }
    }
}

fn now_ts() -> Value {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Value::Timestamp(TimeUnit::Second, secs)
}

fn ts_value(ts: Option<i64>) -> Value {
    ts.map_or(Value::Null, |t| Value::Timestamp(TimeUnit::Second, t))
}

fn ingest_file(
    store: &mut Store,
    local: &Path,
    origin: Option<Origin<'_>>,
    opts: &IngestOptions,
    ctx: &Context,
    progress: &mut dyn FnMut(Progress<'_>),
    cancel: &AtomicBool,
) -> Result<FileReport> {
    let started = Instant::now();
    let conn = &store.conn;
    // `path` is the name the file goes by in the database, reports and
    // errors: the path on the server for a fetched file.
    let (source_text, path) = match origin {
        Some(origin) => (origin.source, PathBuf::from(origin.path)),
        None => (
            "local",
            std::path::absolute(local).unwrap_or_else(|_| local.to_path_buf()),
        ),
    };
    let path = &path;
    let path_text = match origin {
        Some(origin) => origin.path.to_owned(),
        None => path.to_string_lossy().into_owned(),
    };

    let probe = read_probe(local, path)?;
    let stored = stored_files(conn)?;

    // Open the file and, when it starts like a file already ingested, skip to
    // where that ingest stopped and check the content before it is the same.
    let mut resumed: Option<&StoredFile> = None;
    let mut opened = None;
    for candidate in head_matches(&probe, &stored) {
        let mut src = source::open(local).map_err(file_error(path))?;
        let mut fp = Fingerprint::new();
        let same = skip_bytes(&mut *src.reader, candidate.bytes_consumed, &mut fp)
            .map_err(file_error(path))?
            && fp.tail_hash() == candidate.tail_hash;
        if same {
            resumed = Some(candidate);
            opened = Some((src, fp));
            break;
        }
    }

    let tx = Transaction::begin(conn)?;
    if let (Some(old), true) = (resumed, opts.force) {
        conn.execute(
            "DELETE FROM requests WHERE file_id = ?",
            params![old.file_id],
        )?;
        conn.execute(
            "DELETE FROM ingested_files WHERE file_id = ?",
            params![old.file_id],
        )?;
    }
    let replaced = resumed.filter(|_| opts.force);
    let resumed = resumed.filter(|_| !opts.force);
    let (mut src, mut fp) = match opened {
        Some(pair) if resumed.is_some() => pair,
        _ => (
            source::open(local).map_err(file_error(path))?,
            Fingerprint::new(),
        ),
    };

    // Format: explicit, else the one the file was first ingested with, else detected.
    let format: Box<dyn LogFormat> = match (&opts.format, resumed) {
        (Some(name), _) => detect::by_name(name)
            .ok_or_else(|| Error::invalid(format!("unknown log format '{name}'")))?,
        (None, Some(old)) if detect::by_name(&old.format).is_some() => {
            detect::by_name(&old.format).expect("checked above")
        }
        _ => {
            let text = String::from_utf8_lossy(&probe);
            let mut lines: Vec<&str> = text.split('\n').collect();
            // The probe may have stopped mid-line.
            if probe.len() == PROBE_BYTES {
                lines.pop();
            }
            if lines.iter().all(|l| l.trim().is_empty()) {
                return Ok(FileReport {
                    path: path.clone(),
                    status: FileStatus::Empty,
                    file_id: None,
                    format: None,
                    gzip: src.gzip,
                    size_bytes: src.size_bytes,
                    rows_ok: 0,
                    rows_bad: 0,
                    rows_skipped: 0,
                    first_ts: None,
                    last_ts: None,
                    bad_samples: Vec::new(),
                    seconds: started.elapsed().as_secs_f64(),
                });
            }
            detect::detect(lines).map_err(|scores| Error::UnknownFormat {
                path: path.clone(),
                tried: scores
                    .iter()
                    .map(|s| format!("{} ({}/{} lines)", s.format, s.parsed, s.lines))
                    .collect::<Vec<_>>()
                    .join(", "),
            })?
        }
    };

    let file_id: i32 = match resumed {
        Some(old) => old.file_id,
        None => conn.query_row(
            "SELECT coalesce(max(file_id), 0) + 1 FROM ingested_files",
            [],
            |r| r.get(0),
        )?,
    };
    let covered = Covered::new(&stored, resumed.or(replaced).map(|f| f.file_id));
    let mut uas = UaCache::load(conn, &ctx.registry)?;
    let mut assessments: HashMap<(usize, IpAddr), Assessment> = HashMap::new();
    let mut clock = LocalClock::new(ctx.timezone);
    // Per address: its text form, its /24 or /48, and where it is.
    let mut ips: HashMap<IpAddr, (String, String, Location)> = HashMap::new();
    let mut days: BTreeSet<i64> = BTreeSet::new();

    let (mut lines, mut rows_ok, mut rows_bad, mut rows_skipped) = (0u64, 0u64, 0u64, 0u64);
    let (mut first_ts, mut last_ts) = (i64::MAX, i64::MIN);
    let mut bad_samples = Vec::new();
    let mut buf: Vec<u8> = Vec::with_capacity(16 * 1024);

    {
        let mut appender = conn.appender("requests")?;
        loop {
            buf.clear();
            let n = src
                .reader
                .read_until(b'\n', &mut buf)
                .map_err(file_error(path))?;
            if n == 0 {
                break;
            }
            // A half-written last line of a live log is left for next time.
            if buf.last() != Some(&b'\n') && !src.gzip {
                break;
            }
            fp.feed(&buf);

            let text = match std::str::from_utf8(&buf) {
                Ok(s) => Cow::Borrowed(s),
                Err(_) => String::from_utf8_lossy(&buf),
            };
            let line = text.trim_end_matches(['\n', '\r']);
            if line.trim().is_empty() {
                continue;
            }
            lines += 1;

            match format.parse(line) {
                Err(e) => {
                    rows_bad += 1;
                    if bad_samples.len() < MAX_BAD_SAMPLES {
                        bad_samples.push(BadLine {
                            reason: e.to_string(),
                            line: line.chars().take(300).collect(),
                        });
                    }
                }
                Ok(parsed) => {
                    let r = parsed.record;
                    if ctx.overlap != OverlapPolicy::Allow && covered.contains(r.ts) {
                        if ctx.overlap == OverlapPolicy::Error {
                            return Err(Error::Overlap {
                                path: path.clone(),
                                with: covered.files_containing(r.ts),
                            });
                        }
                        rows_skipped += 1;
                    } else {
                        let ts_local = r.ts + clock.offset(r.ts);
                        days.insert(ts_local.div_euclid(86_400));
                        first_ts = first_ts.min(r.ts);
                        last_ts = last_ts.max(r.ts);

                        let ip = r.ip.to_canonical();
                        if ips.len() >= IP_CACHE_LIMIT {
                            ips.clear();
                        }
                        let (ip_text, net_text, location) = ips
                            .entry(ip)
                            .or_insert_with(|| (ip.to_string(), ip_net(ip), ctx.geo.lookup(ip)));

                        let (method, req_path, query, protocol, raw_request) = match r.request {
                            Request::Parsed {
                                method,
                                path,
                                query,
                                protocol,
                            } => (Some(method), Some(path), query, protocol, None),
                            Request::Malformed(raw) => (None, None, None, None, Some(raw)),
                        };
                        let ref_host = r.referer.and_then(referer_host);
                        let (ua_id, ua_category, operator) =
                            uas.get(r.user_agent.unwrap_or(""), &ctx.registry);
                        // A user agent that claims a registered bot is checked against
                        // that operator's published ranges as of the request's date.
                        let bot_verify = operator.map(|operator| {
                            if assessments.len() >= IP_CACHE_LIMIT {
                                assessments.clear();
                            }
                            assessments
                                .entry((operator, ip))
                                .or_insert_with(|| ctx.ranges.assess(operator, ip))
                                .verdict(r.ts.div_euclid(86_400))
                                .as_str()
                        });
                        let source_type = ctx.rules.source_type(ref_host.as_deref(), query);
                        let is_page = method == Some("GET")
                            && matches!(r.status, 200 | 304)
                            && ua_category == UaCategory::Browser
                            && req_path.is_some_and(|p| !ctx.rules.is_asset(p));

                        appender.append_row(params![
                            Value::Timestamp(TimeUnit::Second, r.ts),
                            Value::Timestamp(TimeUnit::Second, ts_local),
                            ip_text.as_str(),
                            net_text.as_str(),
                            None::<&str>, // peer_ip
                            method,
                            req_path,
                            query,
                            protocol,
                            raw_request,
                            r.status,
                            r.bytes,
                            r.referer,
                            ref_host.as_deref(),
                            ua_id,
                            r.request_ms,
                            r.vhost,
                            location.country.as_deref(),
                            location.asn,
                            location.asn_org.as_deref(),
                            bot_verify,
                            source_type.as_str(),
                            is_page,
                            file_id,
                        ])?;
                        rows_ok += 1;
                    }
                }
            }

            if lines == WRONG_FORMAT_WINDOW {
                check_format(path, format.name(), rows_bad, lines)?;
            }
            if lines % PROGRESS_EVERY_LINES == 0 {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                progress(Progress {
                    path,
                    bytes_done: src.consumed.load(Ordering::Relaxed),
                    bytes_total: src.size_bytes,
                    rows: rows_ok,
                });
            }
        }
        if lines < WRONG_FORMAT_WINDOW {
            check_format(path, format.name(), rows_bad, lines)?;
        }
        appender.flush()?;
    }

    let mut report = FileReport {
        path: path.clone(),
        status: FileStatus::Unchanged,
        file_id: Some(file_id),
        format: Some(format.name().to_owned()),
        gzip: src.gzip,
        size_bytes: src.size_bytes,
        rows_ok,
        rows_bad,
        rows_skipped,
        first_ts: (rows_ok > 0).then_some(first_ts),
        last_ts: (rows_ok > 0).then_some(last_ts),
        bad_samples,
        seconds: 0.0,
    };

    // Nothing new in a file we already have: leave the database untouched.
    if lines == 0 && resumed.is_some() {
        drop(tx);
        report.seconds = started.elapsed().as_secs_f64();
        return Ok(report);
    }
    if lines == 0 {
        drop(tx);
        report.status = FileStatus::Empty;
        report.file_id = None;
        report.seconds = started.elapsed().as_secs_f64();
        return Ok(report);
    }

    uas.flush(conn)?;
    match resumed {
        Some(old) => {
            report.status = FileStatus::Resumed;
            let first = [old.first_ts, report.first_ts].into_iter().flatten().min();
            let last = [old.last_ts, report.last_ts].into_iter().flatten().max();
            conn.execute(
                "UPDATE ingested_files SET source = ?, path = ?, size_bytes = ?, bytes_consumed = ?,
                        head_hash = ?, tail_hash = ?, first_ts = ?, last_ts = ?, rows_ok = ?,
                        rows_bad = ?, rows_skipped = ?, ingested_at = ?
                 WHERE file_id = ?",
                params![
                    source_text,
                    path_text,
                    src.size_bytes,
                    fp.total,
                    fp.head_hash(),
                    fp.tail_hash(),
                    ts_value(first),
                    ts_value(last),
                    old.rows_ok + rows_ok,
                    old.rows_bad + rows_bad,
                    old.rows_skipped + rows_skipped,
                    now_ts(),
                    file_id,
                ],
            )?;
        }
        None => {
            report.status = FileStatus::Ingested;
            conn.execute(
                "INSERT INTO ingested_files VALUES (?, ?, ?, ?, ?, ?, ?, ?, false, ?, ?, ?, ?, ?, ?)",
                params![
                    file_id,
                    source_text,
                    path_text,
                    src.size_bytes,
                    fp.total,
                    fp.head_hash(),
                    fp.tail_hash(),
                    format.name(),
                    ts_value(report.first_ts),
                    ts_value(report.last_ts),
                    rows_ok,
                    rows_bad,
                    rows_skipped,
                    now_ts(),
                ],
            )?;
        }
    }

    let pruned_before = store
        .meta("pruned_before")?
        .and_then(|d| d.parse::<NaiveDate>().ok());
    rebuild_rollups(conn, &days, pruned_before)?;
    tx.commit()?;

    progress(Progress {
        path,
        bytes_done: src.size_bytes,
        bytes_total: src.size_bytes,
        rows: rows_ok,
    });
    report.seconds = started.elapsed().as_secs_f64();
    Ok(report)
}

fn check_format(path: &Path, format: &str, failed: u64, checked: u64) -> Result<()> {
    if checked > 0 && failed * 5 > checked {
        return Err(Error::WrongFormat {
            path: path.to_path_buf(),
            format: format.to_owned(),
            failed,
            checked,
        });
    }
    Ok(())
}

/// Rebuilds the daily rollups for the given days (days since the Unix epoch,
/// in the site's time zone) from raw rows.
///
/// Days before `pruned_before` are left alone: their raw rows are gone, so a
/// rebuild would replace a complete rollup with a partial one.
pub(crate) fn rebuild_rollups(
    conn: &Connection,
    days: &BTreeSet<i64>,
    pruned_before: Option<NaiveDate>,
) -> Result<()> {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid date");
    let dates: Vec<NaiveDate> = days
        .iter()
        .filter_map(|d| epoch.checked_add_signed(chrono::Duration::days(*d)))
        .filter(|d| pruned_before.is_none_or(|cutoff| *d >= cutoff))
        .collect();
    let (Some(first), Some(last)) = (dates.first(), dates.last()) else {
        return Ok(());
    };
    let list = dates
        .iter()
        .map(|d| format!("DATE '{d}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let next = last.succ_opt().unwrap_or(*last);
    // The range lets DuckDB skip row groups; the list picks the exact days.
    let scope = format!(
        "r.ts_local >= TIMESTAMP '{first} 00:00:00' AND r.ts_local < TIMESTAMP '{next} 00:00:00' \
         AND CAST(r.ts_local AS DATE) IN ({list})"
    );

    conn.execute_batch(&format!(
        "DELETE FROM daily_totals WHERE day IN ({list});
         DELETE FROM daily_sources WHERE day IN ({list});
         DELETE FROM daily_bots WHERE day IN ({list});

         INSERT INTO daily_bots
         SELECT CAST(r.ts_local AS DATE) AS day, u.bot_key, r.bot_verify, count(*),
                coalesce(sum(r.bytes), 0), count(DISTINCT r.path),
                count(*) FILTER (WHERE r.status >= 400)
         FROM requests r JOIN user_agents u USING (ua_id)
         WHERE {scope} AND u.bot_key IS NOT NULL
         GROUP BY ALL;

         INSERT INTO daily_totals
         SELECT CAST(r.ts_local AS DATE) AS day,
                count(*),
                count(*) FILTER (WHERE r.is_page),
                coalesce(sum(r.bytes), 0),
                count(DISTINCT r.ip),
                count(*) FILTER (WHERE r.status BETWEEN 200 AND 299),
                count(*) FILTER (WHERE r.status BETWEEN 300 AND 399),
                count(*) FILTER (WHERE r.status BETWEEN 400 AND 499),
                count(*) FILTER (WHERE r.status BETWEEN 500 AND 599),
                count(*) FILTER (WHERE u.category = 'browser'),
                count(*) FILTER (WHERE u.category IN ('bot', 'tool', 'library'))
         FROM requests r LEFT JOIN user_agents u USING (ua_id)
         WHERE {scope}
         GROUP BY day;

         INSERT INTO daily_sources
         WITH base AS (
           SELECT CAST(r.ts_local AS DATE) AS day, r.source_type, r.referer_host, r.country, r.asn,
                  count(*) AS hits, count(*) FILTER (WHERE r.is_page) AS pages
           FROM requests r
           WHERE {scope}
           GROUP BY ALL
         ),
         ranked AS (
           SELECT day, referer_host,
                  row_number() OVER (PARTITION BY day ORDER BY sum(hits) DESC, referer_host) AS rn
           FROM base WHERE referer_host IS NOT NULL
           GROUP BY day, referer_host
         )
         SELECT b.day, b.source_type,
                CASE WHEN b.referer_host IS NULL THEN NULL
                     WHEN k.rn <= 500 THEN b.referer_host ELSE '(other)' END AS host,
                CASE WHEN k.rn > 500 THEN NULL ELSE b.country END AS country,
                CASE WHEN k.rn > 500 THEN NULL ELSE b.asn END AS asn,
                sum(b.hits), sum(b.pages)
         FROM base b LEFT JOIN ranked k ON k.day = b.day AND k.referer_host = b.referer_host
         GROUP BY ALL;"
    ))?;
    Ok(())
}

#[cfg(test)]
#[path = "ingest_tests.rs"]
mod tests;
