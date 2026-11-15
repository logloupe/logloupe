//! Country and network (ASN) lookup from local `.mmdb` files (spec 5.2).
//!
//! Files live in `<data-dir>/geo/`. A user's own MaxMind GeoLite2 files take
//! priority; otherwise the free DB-IP Lite files are used, which the tool can
//! download itself. With no files at all, lookups return nothing and ingest
//! still works.
//!
//! DB-IP Lite is licensed CC BY 4.0: show "IP geolocation by DB-IP" with a
//! link to https://db-ip.com wherever its data is displayed.

use std::io::Read;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Datelike, Months, NaiveDate, Utc};
use maxminddb::Reader;
use serde::{Deserialize, Serialize};

use crate::bots::ranges::USER_AGENT;
use crate::error::{Error, Result};

pub const DBIP_ATTRIBUTION: &str = "IP geolocation by DB-IP (https://db-ip.com)";

const MAXMIND_COUNTRY: &str = "GeoLite2-Country.mmdb";
const MAXMIND_ASN: &str = "GeoLite2-ASN.mmdb";
const DBIP_COUNTRY: &str = "dbip-country-lite.mmdb";
const DBIP_ASN: &str = "dbip-asn-lite.mmdb";

/// Largest database accepted from a download, decompressed.
const MAX_DATABASE_BYTES: u64 = 512 << 20;

#[derive(Deserialize)]
struct CountryRecord {
    country: Option<CountryPart>,
}

#[derive(Deserialize)]
struct CountryPart {
    iso_code: Option<String>,
}

#[derive(Deserialize)]
struct AsnRecord {
    autonomous_system_number: Option<u32>,
    autonomous_system_organization: Option<String>,
}

/// What is known about one address.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Location {
    /// ISO 3166-1 alpha-2 country code.
    pub country: Option<String>,
    pub asn: Option<u32>,
    pub asn_org: Option<String>,
}

pub fn geo_dir(data_root: &Path) -> PathBuf {
    data_root.join("geo")
}

/// The file to use for one kind of lookup: the user's MaxMind file if present,
/// else the DB-IP one.
fn pick(data_root: &Path, maxmind: &str, dbip: &str) -> Option<(PathBuf, &'static str)> {
    let dir = geo_dir(data_root);
    [(maxmind, "MaxMind GeoLite2"), (dbip, "DB-IP Lite")]
        .into_iter()
        .map(|(name, provider)| (dir.join(name), provider))
        .find(|(path, _)| path.is_file())
}

/// The loaded databases. Either may be missing.
pub struct GeoDb {
    country: Option<Reader<Vec<u8>>>,
    asn: Option<Reader<Vec<u8>>>,
}

impl GeoDb {
    /// Loads whatever databases are present. A file that cannot be read is
    /// treated as missing: ingest must never fail because of geo data.
    pub fn load(data_root: &Path) -> Self {
        let open = |maxmind: &str, dbip: &str| {
            pick(data_root, maxmind, dbip).and_then(|(path, _)| Reader::open_readfile(path).ok())
        };
        GeoDb {
            country: open(MAXMIND_COUNTRY, DBIP_COUNTRY),
            asn: open(MAXMIND_ASN, DBIP_ASN),
        }
    }

    pub fn none() -> Self {
        GeoDb {
            country: None,
            asn: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.country.is_none() && self.asn.is_none()
    }

    pub fn lookup(&self, ip: IpAddr) -> Location {
        let mut location = Location::default();
        if let Some(reader) = &self.country
            && let Ok(result) = reader.lookup(ip)
            && let Ok(Some(record)) = result.decode::<CountryRecord>()
        {
            location.country = record
                .country
                .and_then(|c| c.iso_code)
                .filter(|code| code.len() == 2)
                .map(|code| code.to_ascii_uppercase());
        }
        if let Some(reader) = &self.asn
            && let Ok(result) = reader.lookup(ip)
            && let Ok(Some(record)) = result.decode::<AsnRecord>()
        {
            location.asn = record.autonomous_system_number;
            location.asn_org = record.autonomous_system_organization;
        }
        location
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GeoFileStatus {
    /// `country` or `asn`.
    pub kind: &'static str,
    pub present: bool,
    /// `DB-IP Lite` or `MaxMind GeoLite2`.
    pub provider: Option<&'static str>,
    pub file: Option<String>,
    pub size_bytes: Option<u64>,
    /// When the provider built the database (UTC, ISO 8601).
    pub built_at: Option<String>,
    /// Older than about five weeks: a newer monthly file should exist.
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeoStatus {
    pub files: Vec<GeoFileStatus>,
    /// Shown wherever DB-IP data is displayed; `None` when it is not in use.
    pub attribution: Option<&'static str>,
}

fn file_status(data_root: &Path, kind: &'static str, maxmind: &str, dbip: &str) -> GeoFileStatus {
    let Some((path, provider)) = pick(data_root, maxmind, dbip) else {
        return GeoFileStatus {
            kind,
            present: false,
            provider: None,
            file: None,
            size_bytes: None,
            built_at: None,
            stale: true,
        };
    };
    let built_at: Option<DateTime<Utc>> = Reader::open_readfile(&path)
        .ok()
        .and_then(|reader| reader.metadata().build_time().ok())
        .map(DateTime::<Utc>::from);
    GeoFileStatus {
        kind,
        present: true,
        provider: Some(provider),
        file: path.file_name().map(|n| n.to_string_lossy().into_owned()),
        size_bytes: std::fs::metadata(&path).ok().map(|m| m.len()),
        built_at: built_at.map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        stale: built_at.is_none_or(|t| Utc::now().signed_duration_since(t).num_days() > 37),
    }
}

pub fn status(data_root: &Path) -> GeoStatus {
    let files = vec![
        file_status(data_root, "country", MAXMIND_COUNTRY, DBIP_COUNTRY),
        file_status(data_root, "asn", MAXMIND_ASN, DBIP_ASN),
    ];
    let uses_dbip = files.iter().any(|f| f.provider == Some("DB-IP Lite"));
    GeoStatus {
        files,
        attribution: uses_dbip.then_some(DBIP_ATTRIBUTION),
    }
}

fn download(url: &str) -> std::result::Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .user_agent(USER_AGENT)
        .build()
        .into();
    let mut response = agent.get(url).call().map_err(|e| e.to_string())?;
    response
        .body_mut()
        .with_config()
        .limit(MAX_DATABASE_BYTES)
        .read_to_vec()
        .map_err(|e| e.to_string())
}

/// Downloads one DB-IP Lite file for the given month, checks that it opens
/// as a database, and swaps it in. The previous file stays if anything fails.
fn fetch_dbip(
    data_root: &Path,
    name: &str,
    target: &str,
    month: NaiveDate,
) -> std::result::Result<(), String> {
    let url = format!(
        "https://download.db-ip.com/free/{name}-{}-{:02}.mmdb.gz",
        month.year(),
        month.month()
    );
    let compressed = download(&url).map_err(|e| format!("{url}: {e}"))?;
    let mut database = Vec::new();
    flate2::read::GzDecoder::new(compressed.as_slice())
        .take(MAX_DATABASE_BYTES)
        .read_to_end(&mut database)
        .map_err(|e| format!("{url}: not a gzip file ({e})"))?;
    Reader::from_source(database.as_slice())
        .map_err(|e| format!("{url}: not a valid database ({e})"))?;

    let dir = geo_dir(data_root);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(target);
    let temp = dir.join(format!("{target}.tmp"));
    std::fs::write(&temp, &database).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, &path).map_err(|e| e.to_string())
}

/// Downloads this month's DB-IP Lite country and ASN databases, falling back
/// to last month's when the new ones are not published yet.
pub fn update_dbip(data_root: &Path) -> Result<GeoStatus> {
    let this_month = Utc::now().date_naive().with_day(1).expect("day 1 exists");
    let last_month = this_month
        .checked_sub_months(Months::new(1))
        .unwrap_or(this_month);
    for (name, target) in [
        ("dbip-country-lite", DBIP_COUNTRY),
        ("dbip-asn-lite", DBIP_ASN),
    ] {
        if let Err(first) = fetch_dbip(data_root, name, target, this_month) {
            fetch_dbip(data_root, name, target, last_month).map_err(|second| {
                Error::invalid(format!(
                    "could not download the {name} database. This month: {first}. Last month: {second}"
                ))
            })?;
        }
    }
    Ok(status(data_root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testing::TempDataDir;

    #[test]
    fn nothing_installed_is_not_an_error() {
        let tmp = TempDataDir::new("geo-empty");
        let geo = GeoDb::load(tmp.0.root());
        assert!(geo.is_empty());
        assert_eq!(
            geo.lookup("203.0.113.7".parse().unwrap()),
            Location::default()
        );
        let s = status(tmp.0.root());
        assert_eq!(s.files.len(), 2);
        assert!(s.files.iter().all(|f| !f.present && f.stale));
        assert_eq!(s.attribution, None);
    }

    #[test]
    fn unreadable_files_are_treated_as_missing() {
        let tmp = TempDataDir::new("geo-bad");
        std::fs::create_dir_all(geo_dir(tmp.0.root())).unwrap();
        std::fs::write(geo_dir(tmp.0.root()).join(DBIP_ASN), b"not a database").unwrap();
        let geo = GeoDb::load(tmp.0.root());
        assert!(geo.is_empty());
        let s = status(tmp.0.root());
        let asn = s.files.iter().find(|f| f.kind == "asn").unwrap();
        assert!(asn.present && asn.built_at.is_none() && asn.stale);
    }
}
