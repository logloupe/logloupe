//! Abuse reports (spec 6.3). So far: the busiest addresses, networks and user agents.

use super::ColumnType::{Datetime, Integer, String as Text};
use super::{ReportParams, ReportResult, Scope, TableSpec, col};
use crate::error::Result;
use crate::store::Store;

const SEEN: &str = "strftime(min(r.ts_local), '%Y-%m-%d %H:%M:%S') AS first_seen,
                    strftime(max(r.ts_local), '%Y-%m-%d %H:%M:%S') AS last_seen";

const ROWS: &str = "requests r LEFT JOIN user_agents u USING (ua_id)";

/// Counts shared by every table: volume, and how much of it was refused.
const COUNTS: &str = "count(*) AS hits,
                      count(*) FILTER (WHERE r.status >= 400) AS errors,
                      count(*) FILTER (WHERE r.status IN (403, 429, 444)) AS blocked,
                      coalesce(sum(r.bytes), 0) AS bytes";

pub fn top_ips(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    let filter = scope.filter.clone();
    scope.notes.push(
        "Peak per minute is the most requests the address (or network) made in any single minute. \
         Blocked counts responses with status 403, 429 or 444; errors counts every status of 400 or above."
            .to_owned(),
    );
    scope.notes.push(
        "Country and network (ASN) come from the geo database in Settings. They are empty for requests \
         ingested before one was installed, until bots are re-checked on the Storage screen."
            .to_owned(),
    );
    scope.notes.push(
        "Use the IP filter above with an address or a /24 to see what one address or network did in the other reports."
            .to_owned(),
    );
    scope.finish(
        "top-ips",
        vec![
            TableSpec {
                name: "by_ip",
                columns: vec![
                    col("ip", Text),
                    col("network", Text),
                    col("country", Text),
                    col("asn", Integer),
                    col("asn_org", Text),
                    col("hits", Integer),
                    col("peak_per_minute", Integer),
                    col("errors", Integer),
                    col("blocked", Integer),
                    col("urls", Integer),
                    col("bytes", Integer),
                    col("agent_type", Text),
                    col("agent", Text),
                    col("user_agents", Integer),
                    col("verify", Text),
                    col("first_seen", Datetime),
                    col("last_seen", Datetime),
                ],
                sql: format!(
                    "WITH base AS (
                       SELECT r.ip, any_value(r.ip_net) AS network, any_value(r.country) AS country,
                              any_value(r.asn) AS asn, any_value(r.asn_org) AS asn_org, {COUNTS},
                              count(DISTINCT coalesce(r.path, r.raw_request)) AS urls,
                              mode(u.category) AS agent_type, mode(u.family) AS agent,
                              count(DISTINCT r.ua_id) AS user_agents,
                              max(r.bot_verify) AS verify, {SEEN}
                       FROM {ROWS} WHERE {filter} GROUP BY r.ip),
                     peaks AS (
                       SELECT ip, max(c) AS peak_per_minute FROM (
                         SELECT r.ip, count(*) AS c FROM requests r
                         WHERE {filter} GROUP BY r.ip, date_trunc('minute', r.ts)) GROUP BY ip)
                     SELECT base.*, peaks.peak_per_minute FROM base JOIN peaks USING (ip)"
                ),
                order_by: "hits DESC, ip",
                totals: Some((
                    vec![col("hits", Integer), col("ips", Integer), col("errors", Integer), col("blocked", Integer)],
                    format!(
                        "SELECT count(*) AS hits, count(DISTINCT r.ip) AS ips,
                                count(*) FILTER (WHERE r.status >= 400) AS errors,
                                count(*) FILTER (WHERE r.status IN (403, 429, 444)) AS blocked
                         FROM requests r WHERE {filter}"
                    ),
                )),
            },
            TableSpec {
                name: "by_network",
                columns: vec![
                    col("network", Text),
                    col("ips", Integer),
                    col("hits", Integer),
                    col("peak_per_minute", Integer),
                    col("errors", Integer),
                    col("blocked", Integer),
                    col("bytes", Integer),
                    col("agent_type", Text),
                    col("agent", Text),
                    col("first_seen", Datetime),
                    col("last_seen", Datetime),
                ],
                sql: format!(
                    "WITH base AS (
                       SELECT r.ip_net AS network, count(DISTINCT r.ip) AS ips, {COUNTS},
                              mode(u.category) AS agent_type, mode(u.family) AS agent, {SEEN}
                       FROM {ROWS} WHERE {filter} GROUP BY r.ip_net),
                     peaks AS (
                       SELECT network, max(c) AS peak_per_minute FROM (
                         SELECT r.ip_net AS network, count(*) AS c FROM requests r
                         WHERE {filter} GROUP BY r.ip_net, date_trunc('minute', r.ts)) GROUP BY network)
                     SELECT base.*, peaks.peak_per_minute FROM base JOIN peaks USING (network)"
                ),
                order_by: "hits DESC, network",
                totals: None,
            },
            TableSpec {
                name: "by_asn",
                columns: vec![
                    col("asn", Integer),
                    col("asn_org", Text),
                    col("ips", Integer),
                    col("hits", Integer),
                    col("errors", Integer),
                    col("blocked", Integer),
                    col("bytes", Integer),
                    col("agent_type", Text),
                    col("agent", Text),
                ],
                sql: format!(
                    "SELECT r.asn, any_value(r.asn_org) AS asn_org, count(DISTINCT r.ip) AS ips, {COUNTS},
                            mode(u.category) AS agent_type, mode(u.family) AS agent
                     FROM {ROWS} WHERE {filter} AND r.asn IS NOT NULL GROUP BY r.asn"
                ),
                order_by: "hits DESC, asn",
                totals: None,
            },
            TableSpec {
                name: "by_country",
                columns: vec![
                    col("country", Text),
                    col("ips", Integer),
                    col("hits", Integer),
                    col("errors", Integer),
                    col("blocked", Integer),
                    col("bytes", Integer),
                ],
                sql: format!(
                    "SELECT r.country, count(DISTINCT r.ip) AS ips, {COUNTS}
                     FROM requests r WHERE {filter} AND r.country IS NOT NULL GROUP BY r.country"
                ),
                order_by: "hits DESC, country",
                totals: None,
            },
            TableSpec {
                name: "by_user_agent",
                columns: vec![
                    col("user_agent", Text),
                    col("agent_type", Text),
                    col("ips", Integer),
                    col("hits", Integer),
                    col("errors", Integer),
                    col("blocked", Integer),
                    col("bytes", Integer),
                ],
                sql: format!(
                    "SELECT coalesce(nullif(u.ua, ''), '(none)') AS user_agent, any_value(u.category) AS agent_type,
                            count(DISTINCT r.ip) AS ips, {COUNTS}
                     FROM {ROWS} WHERE {filter} GROUP BY 1"
                ),
                order_by: "hits DESC, user_agent",
                totals: None,
            },
            TableSpec {
                name: "by_path",
                columns: vec![
                    col("path", Text),
                    col("ips", Integer),
                    col("hits", Integer),
                    col("errors", Integer),
                    col("blocked", Integer),
                    col("bytes", Integer),
                ],
                sql: format!(
                    "SELECT coalesce(r.path, r.raw_request) AS path, count(DISTINCT r.ip) AS ips, {COUNTS}
                     FROM requests r WHERE {filter} GROUP BY 1"
                ),
                order_by: "hits DESC, path",
                totals: None,
            },
        ],
    )
}

/// Referrers exactly as sent, internal ones included. Bots often fake a
/// referrer that no browser would send (the bare site address with no
/// trailing slash, for example), which makes them easy to pick out here.
pub fn referrers(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    let filter = scope.filter.clone();
    scope.notes.push(
        "Referrers are shown exactly as the client sent them and every request is counted, including \
         the site's own pages. A browser's referrer for a site's home page ends in a slash, so the bare \
         address without one is usually an automated client."
            .to_owned(),
    );
    scope.notes.push(
        "Blocked counts responses with status 403, 429 or 444; errors counts every status of 400 or above."
            .to_owned(),
    );
    let columns = |first: &'static str| {
        vec![
            col(first, Text),
            col("source_type", Text),
            col("ips", Integer),
            col("hits", Integer),
            col("pages", Integer),
            col("errors", Integer),
            col("blocked", Integer),
            col("bytes", Integer),
            col("agent_type", Text),
            col("agent", Text),
        ]
    };
    let select = |expr: &str, name: &str| {
        format!(
            "SELECT {expr} AS {name}, mode(r.source_type) AS source_type, count(DISTINCT r.ip) AS ips,
                    {COUNTS}, count(*) FILTER (WHERE r.is_page) AS pages,
                    mode(u.category) AS agent_type, mode(u.family) AS agent
             FROM {ROWS} WHERE {filter} GROUP BY 1"
        )
    };
    scope.finish(
        "referrers",
        vec![
            TableSpec {
                name: "by_referrer",
                columns: columns("referrer"),
                sql: select("coalesce(r.referer, '(none)')", "referrer"),
                order_by: "hits DESC, referrer",
                totals: None,
            },
            TableSpec {
                name: "by_host",
                columns: columns("referrer_host"),
                sql: select("coalesce(r.referer_host, '(none)')", "referrer_host"),
                order_by: "hits DESC, referrer_host",
                totals: None,
            },
        ],
    )
}
