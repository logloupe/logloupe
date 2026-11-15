//! Block lists: addresses and networks written out as server configuration.
//! The tool never applies a block itself; it only produces text to paste.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::str::FromStr;

use serde::Deserialize;

use super::{ReportParams, Scope};
use crate::error::{Error, Result};
use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    /// One address or CIDR per line.
    Plain,
    /// `deny 203.0.113.7;`
    Nginx,
    /// `Require not ip 203.0.113.7`, for use inside `<RequireAll>`.
    Apache,
}

impl FromStr for Style {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "plain" => Ok(Style::Plain),
            "nginx" => Ok(Style::Nginx),
            "apache" => Ok(Style::Apache),
            _ => Err(Error::invalid(format!(
                "invalid block list style '{s}': expected plain, nginx or apache"
            ))),
        }
    }
}

/// What to list: single addresses, or their /24 (IPv4) and /48 (IPv6) networks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum By {
    Ip,
    Network,
}

impl FromStr for By {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "ip" => Ok(By::Ip),
            "network" => Ok(By::Network),
            _ => Err(Error::invalid(format!(
                "invalid block list grouping '{s}': expected ip or network"
            ))),
        }
    }
}

/// Parses an address or CIDR and writes it back out in canonical form. The
/// output goes into server configuration, so nothing is ever passed through
/// as text: an entry is either a valid address or network, or it is refused.
fn canonical(entry: &str) -> Option<(u8, IpAddr, String)> {
    let entry = entry.trim();
    let (addr, prefix) = match entry.split_once('/') {
        Some((addr, prefix)) => (addr, Some(prefix.parse::<u8>().ok()?)),
        None => (entry, None),
    };
    let ip = addr.parse::<IpAddr>().ok()?.to_canonical();
    if is_local(ip) {
        return None;
    }
    let bits = if ip.is_ipv4() { 32 } else { 128 };
    let text = match prefix {
        Some(prefix) if prefix > bits => return None,
        // Refuse anything broad enough to block a large part of the internet.
        Some(prefix) if prefix < if ip.is_ipv4() { 8 } else { 16 } => return None,
        Some(prefix) if prefix == bits => ip.to_string(),
        Some(prefix) => format!("{ip}/{prefix}"),
        None => ip.to_string(),
    };
    Some((u8::from(!ip.is_ipv4()), ip, text))
}

/// The server itself and private networks. Blocking these breaks health
/// checks, cache warmers and anything else running on or beside the server,
/// so they never go into a block list.
fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                // Unique local (fc00::/7) and link-local (fe80::/10).
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Splits entries into those safe to block and those that must be left out:
/// any address or network with verified bot traffic in this site's data
/// (blocking Googlebot by volume would be a costly mistake), and the server's
/// own or private addresses. Returns `(kept, left_out)`.
pub fn protect(store: &Store, entries: &[String]) -> Result<(Vec<String>, Vec<String>)> {
    let mut stmt = store.conn.prepare(
        "SELECT DISTINCT ip FROM requests WHERE bot_verify = 'verified'
         UNION SELECT DISTINCT ip_net FROM requests WHERE bot_verify = 'verified'",
    )?;
    let verified: std::collections::HashSet<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<duckdb::Result<_>>()?;
    let (mut kept, mut left_out) = (Vec::new(), Vec::new());
    for entry in entries {
        let local = entry
            .split('/')
            .next()
            .and_then(|a| a.trim().parse::<IpAddr>().ok())
            .is_some_and(|ip| is_local(ip.to_canonical()));
        if local || verified.contains(entry.trim()) {
            left_out.push(entry.clone());
        } else {
            kept.push(entry.clone());
        }
    }
    Ok((kept, left_out))
}

/// Formats entries as a block list: validated, de-duplicated, IPv4 before
/// IPv6, in address order. Fails if any entry is not a public address or
/// network.
pub fn format(entries: &[String], style: Style) -> Result<String> {
    let mut sorted = BTreeSet::new();
    for entry in entries {
        let parsed = canonical(entry).ok_or_else(|| {
            Error::invalid(format!(
                "'{entry}' cannot go in a block list: it is not a public IP address or network"
            ))
        })?;
        sorted.insert(parsed);
    }
    let mut out = String::new();
    for (_, _, text) in &sorted {
        match style {
            Style::Plain => out.push_str(text),
            Style::Nginx => out.push_str(&format!("deny {text};")),
            Style::Apache => out.push_str(&format!("Require not ip {text}")),
        }
        out.push('\n');
    }
    Ok(out)
}

/// The busiest addresses or networks matching the report filters, with at
/// least `min_hits` requests, busiest first. Verified bots and the server's
/// own addresses are never included (see [`protect`]).
pub fn collect(store: &Store, params: &ReportParams, by: By, min_hits: u64) -> Result<Vec<String>> {
    let scope = Scope::new(store, params)?;
    let column = match by {
        By::Ip => "r.ip",
        By::Network => "r.ip_net",
    };
    let targets: Vec<String> = {
        let mut stmt = scope.conn().prepare(&format!(
            "SELECT {column} AS target, count(*) AS hits FROM requests r WHERE {}
             GROUP BY 1 HAVING count(*) >= {min_hits} ORDER BY hits DESC, target",
            scope.filter
        ))?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<duckdb::Result<_>>()?
    };
    let (mut kept, _) = protect(store, &targets)?;
    kept.truncate(params.limit as usize);
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(entries: &[&str], style: Style) -> Result<String> {
        format(
            &entries.iter().map(|e| (*e).to_owned()).collect::<Vec<_>>(),
            style,
        )
    }

    #[test]
    fn styles_sorting_and_duplicates() {
        let entries = [
            "203.0.113.9",
            "2001:DB8::1",
            "198.51.100.0/24",
            "203.0.113.9",
            "::ffff:192.0.2.7",
            "192.0.2.50/32",
        ];
        assert_eq!(
            list(&entries, Style::Plain).unwrap(),
            "192.0.2.7\n192.0.2.50\n198.51.100.0/24\n203.0.113.9\n2001:db8::1\n"
        );
        assert_eq!(
            list(&["203.0.113.9", "2001:db8:1::/48"], Style::Nginx).unwrap(),
            "deny 203.0.113.9;\ndeny 2001:db8:1::/48;\n"
        );
        assert_eq!(
            list(&["198.51.100.0/24"], Style::Apache).unwrap(),
            "Require not ip 198.51.100.0/24\n"
        );
        assert_eq!(list(&[], Style::Plain).unwrap(), "");
    }

    #[test]
    fn refuses_anything_that_is_not_an_address() {
        for bad in [
            "203.0.113.9; include /etc/passwd",
            "all",
            "203.0.113.9\ndeny all",
            "example.com",
            "203.0.113.0/33",
            "0.0.0.0/0",
            "10.0.0.0/7",
            "::/0",
            "",
            // The server itself and private networks.
            "127.0.0.1",
            "127.0.0.0/24",
            "10.1.2.3",
            "192.168.1.0/24",
            "::1",
            "fd00::1",
        ] {
            assert!(list(&[bad], Style::Nginx).is_err(), "{bad:?}");
        }
    }
}
