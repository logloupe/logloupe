//! Storage: one DuckDB file per site inside a data directory.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use duckdb::{AccessMode, Config, Connection, OptionalExt, params};

use crate::error::{Error, Result};

pub mod settings;

pub use settings::{ClientIpMode, OverlapPolicy, Retention, SettingsPatch, SiteSettings};

pub const SCHEMA_VERSION: u32 = 1;

const SCHEMA: &str = r#"
CREATE TABLE meta (key VARCHAR PRIMARY KEY, value VARCHAR);

CREATE TABLE ingested_files (
  file_id        INTEGER PRIMARY KEY,
  source         VARCHAR,
  path           VARCHAR,
  size_bytes     UBIGINT,
  bytes_consumed UBIGINT,
  head_hash      VARCHAR,
  tail_hash      VARCHAR,
  format         VARCHAR,
  proxied        BOOLEAN,
  first_ts       TIMESTAMP,
  last_ts        TIMESTAMP,
  rows_ok        UBIGINT,
  rows_bad       UBIGINT,
  rows_skipped   UBIGINT,
  ingested_at    TIMESTAMP
);

CREATE TABLE user_agents (
  ua_id     UINTEGER PRIMARY KEY,
  ua        VARCHAR,
  category  VARCHAR,
  family    VARCHAR,
  bot_key   VARCHAR
);

CREATE TABLE requests (
  ts            TIMESTAMP,
  ts_local      TIMESTAMP,
  ip            VARCHAR,
  ip_net        VARCHAR,
  peer_ip       VARCHAR,
  method        VARCHAR,
  path          VARCHAR,
  query         VARCHAR,
  protocol      VARCHAR,
  raw_request   VARCHAR,
  status        USMALLINT,
  bytes         UBIGINT,
  referer       VARCHAR,
  referer_host  VARCHAR,
  ua_id         UINTEGER,
  request_ms    UINTEGER,
  vhost         VARCHAR,
  country       VARCHAR,
  asn           UINTEGER,
  asn_org       VARCHAR,
  bot_verify    VARCHAR,
  source_type   VARCHAR,
  is_page       BOOLEAN,
  file_id       INTEGER
);

CREATE TABLE known_urls (
  path       VARCHAR,
  origin     VARCHAR,
  lastmod    TIMESTAMP,
  added_at   TIMESTAMP
);

CREATE TABLE daily_totals  (day DATE, requests UBIGINT, pages UBIGINT, bytes UBIGINT,
                            unique_ips UBIGINT, s2xx UBIGINT, s3xx UBIGINT,
                            s4xx UBIGINT, s5xx UBIGINT, human UBIGINT, bot UBIGINT);
CREATE TABLE daily_bots    (day DATE, bot_key VARCHAR, bot_verify VARCHAR,
                            hits UBIGINT, bytes UBIGINT, urls UBIGINT, errors UBIGINT);
CREATE TABLE daily_sources (day DATE, source_type VARCHAR, referer_host VARCHAR,
                            country VARCHAR, asn UINTEGER, hits UBIGINT, pages UBIGINT);
"#;

/// The data directory: `sites/`, `tmp/`, and later `ranges/` and `geo/`.
#[derive(Debug, Clone)]
pub struct DataDir {
    root: PathBuf,
}

impl DataDir {
    /// Resolution order: the explicit path, `LOGLOUPE_DATA_DIR`, then the
    /// OS-appropriate per-user data directory.
    pub fn resolve(explicit: Option<PathBuf>) -> Result<Self> {
        let root = explicit
            .or_else(|| std::env::var_os("LOGLOUPE_DATA_DIR").map(PathBuf::from))
            .or_else(default_data_dir)
            .ok_or_else(|| {
                Error::invalid("could not work out a data directory; pass --data-dir")
            })?;
        Ok(DataDir { root })
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        DataDir { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn sites_dir(&self) -> PathBuf {
        self.root.join("sites")
    }

    fn temp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    pub fn site_path(&self, slug: &str) -> PathBuf {
        self.sites_dir().join(format!("{slug}.duckdb"))
    }

    /// False for anything that is not a valid site name, so a name such as
    /// `../other` can never make the tool open or delete a file elsewhere.
    pub fn site_exists(&self, slug: &str) -> bool {
        validate_slug(slug).is_ok() && self.site_path(slug).is_file()
    }

    /// Slugs of every site, sorted.
    pub fn list_sites(&self) -> Result<Vec<String>> {
        let mut slugs = Vec::new();
        let entries = match std::fs::read_dir(self.sites_dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(slugs),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "duckdb")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                slugs.push(stem.to_owned());
            }
        }
        slugs.sort();
        Ok(slugs)
    }

    /// Bytes on disk for one site (database plus write-ahead log).
    pub fn site_size(&self, slug: &str) -> u64 {
        if validate_slug(slug).is_err() {
            return 0;
        }
        let db = self.site_path(slug);
        let wal = wal_path(&db);
        [db, wal]
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum()
    }

    /// Deletes a site's database. Frees the disk space immediately.
    pub fn remove_site(&self, slug: &str) -> Result<()> {
        let db = self.site_path(slug);
        if !self.site_exists(slug) {
            return Err(Error::SiteNotFound(slug.to_owned()));
        }
        std::fs::remove_file(&db).map_err(|e| {
            if is_sharing_violation(&e) {
                Error::SiteLocked(slug.to_owned())
            } else {
                Error::File {
                    path: db.clone(),
                    source: e,
                }
            }
        })?;
        let _ = std::fs::remove_file(wal_path(&db));
        Ok(())
    }
}

/// The per-user data directory. Data stored under the project's earlier
/// working name is moved across the first time, or used in place if it
/// cannot be moved.
fn default_data_dir() -> Option<PathBuf> {
    let current = directories::ProjectDirs::from("", "", "logloupe")?
        .data_dir()
        .to_path_buf();
    if current.exists() {
        return Some(current);
    }
    let legacy =
        directories::ProjectDirs::from("", "", "log-analyser").map(|d| d.data_dir().to_path_buf());
    if let Some(legacy) = legacy.filter(|l| l.is_dir()) {
        let moved = current
            .parent()
            .is_some_and(|parent| std::fs::create_dir_all(parent).is_ok())
            && std::fs::rename(&legacy, &current).is_ok();
        if !moved {
            return Some(legacy);
        }
    }
    Some(current)
}

fn wal_path(db: &Path) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(".wal");
    PathBuf::from(name)
}

fn is_sharing_violation(e: &std::io::Error) -> bool {
    // ERROR_SHARING_VIOLATION on Windows.
    cfg!(windows) && e.raw_os_error() == Some(32)
}

pub fn validate_slug(slug: &str) -> Result<()> {
    let bytes = slug.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes.iter().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')
        });
    if valid {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "invalid site slug '{slug}': use lowercase letters, digits, '-', '_' and '.', starting with a letter or digit"
        )))
    }
}

#[derive(Debug, Clone)]
pub struct OpenOptions {
    pub memory_limit_mb: u64,
    /// `None` means cores − 1, minimum 1.
    pub threads: Option<usize>,
    /// How long to keep retrying when another process holds the file.
    pub wait_lock: Duration,
}

impl Default for OpenOptions {
    fn default() -> Self {
        OpenOptions {
            memory_limit_mb: 512,
            threads: None,
            wait_lock: Duration::ZERO,
        }
    }
}

impl OpenOptions {
    fn thread_count(&self) -> usize {
        self.threads
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(|n| n.get().saturating_sub(1))
                    .unwrap_or(1)
            })
            .max(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    ReadOnly,
    ReadWrite,
}

/// An open site database.
pub struct Store {
    pub(crate) conn: Connection,
    slug: String,
    mode: Mode,
    data_root: PathBuf,
}

impl Store {
    /// Creates a new site with the given settings and leaves it open read-write.
    pub fn create(
        dir: &DataDir,
        slug: &str,
        settings: &SiteSettings,
        opts: &OpenOptions,
    ) -> Result<Store> {
        validate_slug(slug)?;
        if dir.site_exists(slug) {
            return Err(Error::SiteExists(slug.to_owned()));
        }
        std::fs::create_dir_all(dir.sites_dir())?;
        let store = Self::connect(dir, slug, Mode::ReadWrite, opts)?;
        let created = store
            .conn
            .execute_batch(&format!("BEGIN; {SCHEMA} COMMIT;"))
            .map_err(Error::from)
            .and_then(|()| store.set_meta("schema_version", &SCHEMA_VERSION.to_string()))
            .and_then(|()| store.set_settings(settings));
        if let Err(e) = created {
            drop(store);
            let _ = dir.remove_site(slug);
            return Err(e);
        }
        Ok(store)
    }

    pub fn open(dir: &DataDir, slug: &str, mode: Mode, opts: &OpenOptions) -> Result<Store> {
        if !dir.site_exists(slug) {
            return Err(Error::SiteNotFound(slug.to_owned()));
        }
        let store = Self::connect(dir, slug, mode, opts)?;
        let version: u32 = store
            .meta("schema_version")?
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| {
                Error::invalid(format!(
                    "site '{slug}' has no schema version; the file is not a site database"
                ))
            })?;
        if version != SCHEMA_VERSION {
            return Err(Error::invalid(format!(
                "site '{slug}' uses schema version {version}, but this build expects {SCHEMA_VERSION}"
            )));
        }
        Ok(store)
    }

    fn connect(dir: &DataDir, slug: &str, mode: Mode, opts: &OpenOptions) -> Result<Store> {
        let path = dir.site_path(slug);
        let temp = dir.temp_dir();
        std::fs::create_dir_all(&temp)?;
        let deadline = Instant::now() + opts.wait_lock;
        loop {
            let config = Config::default()
                .access_mode(match mode {
                    Mode::ReadOnly => AccessMode::ReadOnly,
                    Mode::ReadWrite => AccessMode::ReadWrite,
                })?
                .max_memory(&format!("{}MB", opts.memory_limit_mb.max(64)))?
                .threads(opts.thread_count() as i64)?
                .with("temp_directory", temp.to_string_lossy())?;
            // Reports only ever read this one database. Turning off external
            // access means that even a query-building mistake could not make
            // DuckDB read or write other files or reach the network.
            let config = match mode {
                Mode::ReadOnly => config.enable_external_access(false)?,
                Mode::ReadWrite => config,
            };
            match Connection::open_with_flags(&path, config) {
                Ok(conn) => {
                    // Appends need not keep the order lines arrived in.
                    conn.execute_batch("SET preserve_insertion_order = false;")?;
                    return Ok(Store {
                        conn,
                        slug: slug.to_owned(),
                        mode,
                        data_root: dir.root().to_path_buf(),
                    });
                }
                Err(e) if is_lock_error(&e) => {
                    if Instant::now() >= deadline {
                        return Err(Error::SiteLocked(slug.to_owned()));
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    pub fn slug(&self) -> &str {
        &self.slug
    }

    /// The data directory this site lives in (for the bot registry and range cache).
    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?, ?)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn settings(&self) -> Result<SiteSettings> {
        let defaults = SiteSettings::default();
        let get = |key: &str| self.meta(key);
        Ok(SiteSettings {
            hosts: match get("hosts")? {
                Some(json) => serde_json::from_str(&json)
                    .map_err(|e| Error::invalid(format!("corrupt 'hosts' setting: {e}")))?,
                None => defaults.hosts,
            },
            timezone: match get("timezone")? {
                Some(tz) => settings::parse_timezone(&tz)?,
                None => defaults.timezone,
            },
            retention: match get("retention")? {
                Some(v) => v.parse()?,
                None => defaults.retention,
            },
            overlap: match get("overlap")? {
                Some(v) => v.parse()?,
                None => defaults.overlap,
            },
            client_ip: match get("client_ip")? {
                Some(v) => v.parse()?,
                None => defaults.client_ip,
            },
            edge_cache: get("edge_cache")?.is_some_and(|v| v == "true"),
        })
    }

    pub fn set_settings(&self, s: &SiteSettings) -> Result<()> {
        self.set_meta(
            "hosts",
            &serde_json::to_string(&s.hosts).expect("strings serialise"),
        )?;
        self.set_meta("timezone", s.timezone.name())?;
        self.set_meta("retention", &s.retention.to_string())?;
        self.set_meta("overlap", &s.overlap.to_string())?;
        self.set_meta("client_ip", &s.client_ip.to_string())?;
        self.set_meta("edge_cache", if s.edge_cache { "true" } else { "false" })?;
        Ok(())
    }

    /// Row count and time coverage of the raw rows.
    pub fn coverage(&self) -> Result<Coverage> {
        Ok(self.conn.query_row(
            "SELECT count(*), epoch(min(ts))::BIGINT, epoch(max(ts))::BIGINT FROM requests",
            [],
            |row| {
                Ok(Coverage {
                    rows: row.get(0)?,
                    first_ts: row.get(1)?,
                    last_ts: row.get(2)?,
                })
            },
        )?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coverage {
    pub rows: u64,
    /// Unix seconds, UTC. `None` when there are no rows.
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
}

/// True when opening failed because another process holds the database file.
fn is_lock_error(e: &duckdb::Error) -> bool {
    let message = e.to_string().to_ascii_lowercase();
    [
        "being used by another process",
        "conflicting lock",
        "could not set lock",
        "already open",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// A fresh data directory under the system temp dir, removed on drop.
    pub struct TempDataDir(pub DataDir);

    impl TempDataDir {
        pub fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("logloupe-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            TempDataDir(DataDir::at(root))
        }
    }

    impl Drop for TempDataDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.root());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::TempDataDir;
    use super::*;

    #[test]
    fn slugs() {
        for ok in ["example", "example.com", "a-b_c", "1site"] {
            assert!(validate_slug(ok).is_ok(), "{ok}");
        }
        for bad in [
            "", "Example", "-x", "a b", "a/b", "../x", "..", "a\\b", "c:x",
        ] {
            assert!(validate_slug(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn create_open_settings_remove() {
        let tmp = TempDataDir::new("store");
        let dir = &tmp.0;
        let opts = OpenOptions::default();
        let settings = SiteSettings {
            hosts: vec!["www.example.com".into()],
            timezone: chrono_tz::Europe::London,
            retention: Retention::Days(30),
            overlap: OverlapPolicy::Skip,
            client_ip: ClientIpMode::Auto,
            edge_cache: true,
        };

        let store = Store::create(dir, "example", &settings, &opts).unwrap();
        assert_eq!(store.settings().unwrap(), settings);
        assert_eq!(
            store.coverage().unwrap(),
            Coverage {
                rows: 0,
                first_ts: None,
                last_ts: None
            }
        );
        drop(store);

        assert!(matches!(
            Store::create(dir, "example", &settings, &opts),
            Err(Error::SiteExists(_))
        ));
        assert!(matches!(
            Store::open(dir, "nope", Mode::ReadOnly, &opts),
            Err(Error::SiteNotFound(_))
        ));
        assert_eq!(dir.list_sites().unwrap(), vec!["example".to_owned()]);
        assert!(dir.site_size("example") > 0);

        let store = Store::open(dir, "example", Mode::ReadOnly, &opts).unwrap();
        assert_eq!(store.settings().unwrap(), settings);
        drop(store);

        // A name that points outside the sites folder is never honoured.
        let outside = dir.root().join("victim.duckdb");
        std::fs::write(&outside, b"keep me").unwrap();
        assert!(!dir.site_exists("../victim"));
        assert!(matches!(
            dir.remove_site("../victim"),
            Err(Error::SiteNotFound(_))
        ));
        assert!(matches!(
            Store::open(dir, "../victim", Mode::ReadOnly, &opts),
            Err(Error::SiteNotFound(_))
        ));
        assert_eq!(dir.site_size("../victim"), 0);
        assert!(outside.is_file());

        dir.remove_site("example").unwrap();
        assert!(dir.list_sites().unwrap().is_empty());
    }
}
