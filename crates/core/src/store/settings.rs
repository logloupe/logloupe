//! Per-site settings, stored in the site's `meta` table.

use std::str::FromStr;

use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Retention {
    /// Analyse then discard: cleared at the start of the next ingest.
    None,
    /// Raw rows older than this many days are deleted after each ingest.
    Days(u32),
    Forever,
}

impl FromStr for Retention {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "none" => Ok(Retention::None),
            "forever" => Ok(Retention::Forever),
            _ => s
                .strip_prefix("days:")
                .and_then(|n| n.parse().ok())
                .filter(|n| *n > 0)
                .map(Retention::Days)
                .ok_or_else(|| {
                    Error::invalid(format!(
                        "invalid retention '{s}': expected forever, none or days:N"
                    ))
                }),
        }
    }
}

impl std::fmt::Display for Retention {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Retention::None => f.write_str("none"),
            Retention::Forever => f.write_str("forever"),
            Retention::Days(n) => write!(f, "days:{n}"),
        }
    }
}

/// What to do with lines that fall inside the time range of a file already ingested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OverlapPolicy {
    Error,
    Skip,
    Allow,
}

impl FromStr for OverlapPolicy {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "error" => Ok(OverlapPolicy::Error),
            "skip" => Ok(OverlapPolicy::Skip),
            "allow" => Ok(OverlapPolicy::Allow),
            _ => Err(Error::invalid(format!(
                "invalid overlap policy '{s}': expected error, skip or allow"
            ))),
        }
    }
}

impl std::fmt::Display for OverlapPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OverlapPolicy::Error => "error",
            OverlapPolicy::Skip => "skip",
            OverlapPolicy::Allow => "allow",
        })
    }
}

/// Which address counts as the client (spec 4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientIpMode {
    Auto,
    Peer,
    Forwarded,
}

impl FromStr for ClientIpMode {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "auto" => Ok(ClientIpMode::Auto),
            "peer" => Ok(ClientIpMode::Peer),
            "forwarded" => Ok(ClientIpMode::Forwarded),
            _ => Err(Error::invalid(format!(
                "invalid client IP mode '{s}': expected auto, peer or forwarded"
            ))),
        }
    }
}

impl std::fmt::Display for ClientIpMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ClientIpMode::Auto => "auto",
            ClientIpMode::Peer => "peer",
            ClientIpMode::Forwarded => "forwarded",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SiteSettings {
    /// The site's own hostnames, lowercase. Referrers from these are `internal`.
    pub hosts: Vec<String>,
    pub timezone: Tz,
    pub retention: Retention,
    pub overlap: OverlapPolicy,
    pub client_ip: ClientIpMode,
    /// The site sits behind a caching CDN, so the log holds only the requests
    /// that reached the origin. Reports say so.
    pub edge_cache: bool,
}

impl Default for SiteSettings {
    fn default() -> Self {
        SiteSettings {
            hosts: Vec::new(),
            timezone: Tz::UTC,
            retention: Retention::Forever,
            overlap: OverlapPolicy::Error,
            client_ip: ClientIpMode::Auto,
            edge_cache: false,
        }
    }
}

/// Changes to a site's settings, as text from the command line or the app.
/// Fields left as `None` keep their current value.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SettingsPatch {
    pub hosts: Option<Vec<String>>,
    pub timezone: Option<String>,
    pub retention: Option<String>,
    pub overlap: Option<String>,
    pub client_ip: Option<String>,
    pub edge_cache: Option<bool>,
}

impl SettingsPatch {
    /// Validates and applies the changes.
    pub fn apply(&self, settings: &mut SiteSettings) -> Result<()> {
        if let Some(hosts) = &self.hosts {
            let mut hosts = hosts
                .iter()
                .map(|h| normalise_host(h))
                .collect::<Result<Vec<_>>>()?;
            hosts.sort();
            hosts.dedup();
            if hosts.is_empty() {
                return Err(Error::invalid(
                    "give at least one hostname, for example example.com and www.example.com",
                ));
            }
            settings.hosts = hosts;
        }
        if let Some(tz) = &self.timezone {
            settings.timezone = parse_timezone(tz)?;
        }
        if let Some(retention) = &self.retention {
            settings.retention = retention.parse()?;
        }
        if let Some(overlap) = &self.overlap {
            settings.overlap = overlap.parse()?;
        }
        if let Some(client_ip) = &self.client_ip {
            settings.client_ip = client_ip.parse()?;
        }
        if let Some(edge_cache) = self.edge_cache {
            settings.edge_cache = edge_cache;
        }
        Ok(())
    }
}

pub fn parse_timezone(name: &str) -> Result<Tz> {
    name.parse().map_err(|_| {
        Error::invalid(format!(
            "unknown time zone '{name}': expected an IANA name such as Europe/London"
        ))
    })
}

/// Lowercases and validates a hostname given on the command line.
pub fn normalise_host(host: &str) -> Result<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let valid = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
    if valid {
        Ok(host)
    } else {
        Err(Error::invalid(format!(
            "invalid hostname '{host}': give the bare host, for example www.example.com"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_round_trip() {
        for s in ["none", "forever", "days:30"] {
            assert_eq!(s.parse::<Retention>().unwrap().to_string(), s);
        }
        for bad in ["", "days", "days:0", "days:x", "30"] {
            assert!(bad.parse::<Retention>().is_err(), "{bad}");
        }
    }

    #[test]
    fn hosts() {
        assert_eq!(
            normalise_host(" WWW.Example.com. ").unwrap(),
            "www.example.com"
        );
        assert!(normalise_host("https://example.com").is_err());
        assert!(normalise_host("").is_err());
    }

    #[test]
    fn timezones() {
        assert!(parse_timezone("Europe/London").is_ok());
        assert!(parse_timezone("Mars/Olympus").is_err());
    }
}
