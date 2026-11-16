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

/// What each response code means, for the codes worth explaining.
const STATUS_MEANING: &str = "CASE r.status
    WHEN 200 THEN 'OK' WHEN 204 THEN 'No content' WHEN 206 THEN 'Partial content'
    WHEN 301 THEN 'Moved permanently' WHEN 302 THEN 'Found (temporary redirect)'
    WHEN 304 THEN 'Not modified' WHEN 307 THEN 'Temporary redirect' WHEN 308 THEN 'Permanent redirect'
    WHEN 400 THEN 'Bad request' WHEN 401 THEN 'Unauthorised' WHEN 403 THEN 'Forbidden'
    WHEN 404 THEN 'Not found' WHEN 405 THEN 'Method not allowed' WHEN 408 THEN 'Request timeout'
    WHEN 410 THEN 'Gone' WHEN 429 THEN 'Too many requests'
    WHEN 444 THEN 'Connection closed by nginx without a response'
    WHEN 499 THEN 'Client gave up before the response (nginx)'
    WHEN 500 THEN 'Internal server error' WHEN 502 THEN 'Bad gateway'
    WHEN 503 THEN 'Service unavailable' WHEN 504 THEN 'Gateway timeout'
    ELSE '' END";

/// Response codes: how many of each, how they moved over time, and what was
/// behind the errors. 499, 502, 503 and 504 together are the server-stress signal.
pub fn status_codes(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    let filter = scope.filter.clone();
    let errors = format!("{filter} AND r.status >= 400");
    let (bucket, bucket_type) = match params.granularity {
        Some(super::Granularity::Hour) => ("date_trunc('hour', r.ts_local)", Datetime),
        _ => ("CAST(r.ts_local AS DATE)", super::ColumnType::Date),
    };
    scope.notes.push(
        "499 means the client disconnected before the server answered, and 502, 503 and 504 mean the \
         server or the application behind it could not answer in time. A rise in those together is the \
         sign of an overloaded server. 444 is nginx deliberately closing the connection, usually from a \
         blocking rule."
            .to_owned(),
    );
    scope.finish(
        "status-codes",
        vec![
            TableSpec {
                name: "by_status",
                columns: vec![
                    col("status", Integer),
                    col("meaning", Text),
                    col("hits", Integer),
                    col("ips", Integer),
                    col("urls", Integer),
                    col("bytes", Integer),
                    col("first_seen", Datetime),
                    col("last_seen", Datetime),
                ],
                sql: format!(
                    "SELECT r.status, {STATUS_MEANING} AS meaning, count(*) AS hits,
                            count(DISTINCT r.ip) AS ips,
                            count(DISTINCT coalesce(r.path, r.raw_request)) AS urls,
                            coalesce(sum(r.bytes), 0) AS bytes, {SEEN}
                     FROM requests r WHERE {filter} GROUP BY r.status"
                ),
                order_by: "hits DESC, status",
                totals: Some((
                    vec![col("hits", Integer), col("errors", Integer), col("server_errors", Integer)],
                    format!(
                        "SELECT count(*) AS hits, count(*) FILTER (WHERE r.status >= 400) AS errors,
                                count(*) FILTER (WHERE r.status >= 500) AS server_errors
                         FROM requests r WHERE {filter}"
                    ),
                )),
            },
            TableSpec {
                name: "series",
                columns: vec![col("bucket", bucket_type), col("status", Text), col("hits", Integer)],
                sql: format!(
                    "SELECT {bucket} AS bucket, CAST(r.status AS VARCHAR) AS status, count(*) AS hits
                     FROM requests r WHERE {filter} GROUP BY 1, 2"
                ),
                order_by: "bucket, hits DESC, status",
                totals: None,
            },
            TableSpec {
                name: "error_paths",
                columns: vec![
                    col("path", Text),
                    col("status", Integer),
                    col("hits", Integer),
                    col("ips", Integer),
                    col("first_seen", Datetime),
                    col("last_seen", Datetime),
                ],
                sql: format!(
                    "SELECT coalesce(r.path, r.raw_request) AS path, r.status, count(*) AS hits,
                            count(DISTINCT r.ip) AS ips, {SEEN}
                     FROM requests r WHERE {errors} GROUP BY 1, 2"
                ),
                order_by: "hits DESC, path, status",
                totals: None,
            },
            TableSpec {
                name: "error_ips",
                columns: vec![
                    col("ip", Text),
                    col("status", Integer),
                    col("hits", Integer),
                    col("urls", Integer),
                    col("country", Text),
                    col("asn_org", Text),
                    col("agent_type", Text),
                    col("agent", Text),
                ],
                sql: format!(
                    "SELECT r.ip, r.status, count(*) AS hits,
                            count(DISTINCT coalesce(r.path, r.raw_request)) AS urls,
                            any_value(r.country) AS country, any_value(r.asn_org) AS asn_org,
                            mode(u.category) AS agent_type, mode(u.family) AS agent
                     FROM {ROWS} WHERE {errors} GROUP BY 1, 2"
                ),
                order_by: "hits DESC, ip, status",
                totals: None,
            },
        ],
    )
}

/// Query strings. A crawler stuck in a loop, or a scraper walking a filter,
/// shows up here as one shape with a huge hit count or an absurd parameter.
pub fn queries(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    let filter = format!("{} AND r.query IS NOT NULL", scope.filter);
    // Values are replaced so that requests differing only in their values
    // group together. A second `?` inside a query is kept: it is a tell-tale.
    let shape = "left(regexp_replace(r.query, '=[^&?]*', '=…', 'g'), 300)";
    scope.notes.push(
        "The shape is the query string with every value replaced by …, so requests that differ only in \
         their values group together. A shape containing a second ? or the same parameter many times \
         is usually a crawler following a broken link."
            .to_owned(),
    );
    scope.finish(
        "queries",
        vec![
            TableSpec {
                name: "by_shape",
                columns: vec![
                    col("path", Text),
                    col("query_shape", Text),
                    col("hits", Integer),
                    col("ips", Integer),
                    col("errors", Integer),
                    col("blocked", Integer),
                    col("bytes", Integer),
                ],
                sql: format!(
                    "SELECT r.path, {shape} AS query_shape, count(DISTINCT r.ip) AS ips, {COUNTS}
                     FROM requests r WHERE {filter} GROUP BY 1, 2"
                ),
                order_by: "hits DESC, path, query_shape",
                totals: Some((
                    vec![col("hits", Integer), col("ips", Integer)],
                    format!("SELECT count(*) AS hits, count(DISTINCT r.ip) AS ips FROM requests r WHERE {filter}"),
                )),
            },
            TableSpec {
                name: "by_param",
                columns: vec![
                    col("parameter", Text),
                    col("hits", Integer),
                    col("ips", Integer),
                    col("paths", Integer),
                    col("longest_value", Integer),
                ],
                sql: format!(
                    "SELECT left(split_part(pair, '=', 1), 80) AS parameter, count(*) AS hits,
                            count(DISTINCT ip) AS ips, count(DISTINCT path) AS paths,
                            max(length(pair) - length(split_part(pair, '=', 1)) - 1) AS longest_value
                     FROM (SELECT r.ip, r.path, unnest(string_split(r.query, '&')) AS pair
                           FROM requests r WHERE {filter})
                     WHERE pair <> '' GROUP BY 1"
                ),
                order_by: "hits DESC, parameter",
                totals: None,
            },
            TableSpec {
                name: "by_path",
                columns: vec![
                    col("path", Text),
                    col("hits", Integer),
                    col("distinct_queries", Integer),
                    col("ips", Integer),
                    col("errors", Integer),
                    col("bytes", Integer),
                ],
                sql: format!(
                    "SELECT r.path, count(DISTINCT r.query) AS distinct_queries, count(DISTINCT r.ip) AS ips,
                            {COUNTS}
                     FROM requests r WHERE {filter} GROUP BY 1"
                ),
                order_by: "hits DESC, path",
                totals: None,
            },
        ],
    )
}

/// The individual requests behind the current filters: the drill-down from
/// every other report. Paged from the database, never loaded whole.
pub fn requests(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    let filter = scope.filter.clone();
    scope.notes.push(
        "Times are in the site's time zone. The original log lines are not stored; these are the parsed fields."
            .to_owned(),
    );
    scope.finish(
        "requests",
        vec![TableSpec {
            name: "requests",
            columns: vec![
                col("time", Datetime),
                col("ip", Text),
                col("country", Text),
                col("asn_org", Text),
                col("method", Text),
                col("path", Text),
                col("query", Text),
                col("status", Integer),
                col("bytes", Integer),
                col("referrer", Text),
                col("user_agent", Text),
                col("agent_type", Text),
                col("verify", Text),
            ],
            sql: format!(
                "SELECT strftime(r.ts_local, '%Y-%m-%d %H:%M:%S') AS time, r.ip, r.country, r.asn_org,
                        r.method, coalesce(r.path, r.raw_request) AS path, r.query, r.status, r.bytes,
                        r.referer AS referrer, nullif(u.ua, '') AS user_agent, u.category AS agent_type,
                        r.bot_verify AS verify
                 FROM {ROWS} WHERE {filter}"
            ),
            order_by: "time DESC, ip",
            totals: Some((
                vec![col("requests", Integer), col("ips", Integer)],
                format!("SELECT count(*) AS requests, count(DISTINCT r.ip) AS ips FROM requests r WHERE {filter}"),
            )),
        }],
    )
}

/// Minutes closer together than this are one incident.
const INCIDENT_GAP_MINUTES: u32 = 5;
/// The busiest incidents kept; more than this is noise, not a list to act on.
const MAX_INCIDENTS: u32 = 500;

/// Burst detection (spec 6.3.1). Counts requests per minute at four levels
/// (one address, its /24 or /48, its provider, and the whole site), flags the
/// minutes above a threshold, and merges neighbouring flagged minutes into
/// incidents.
///
/// A network is only flagged when at least two of its addresses are active in
/// the minute, and a provider when at least two of its networks are, so one
/// noisy address does not show up three times.
pub fn bursts(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    let filter = scope.filter.clone();
    let conn = scope.conn();

    // One pass over the requests: how many each address made in each minute.
    // Every level below is derived from this, so nothing is scanned twice and
    // no distinct counts are needed.
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE loga_ip_minutes AS
           SELECT r.ip, any_value(r.ip_net) AS ip_net,
                  'AS' || CAST(any_value(r.asn) AS VARCHAR) AS provider,
                  date_trunc('minute', r.ts) AS m, count(*) AS c
           FROM requests r WHERE {filter} GROUP BY r.ip, date_trunc('minute', r.ts);"
    ))?;

    // "Normal" is the site's median requests per minute over the range.
    let median: f64 = conn.query_row(
        "SELECT coalesce(median(c), 0) FROM (SELECT sum(c) AS c FROM loga_ip_minutes GROUP BY m)",
        [],
        |row| row.get(0),
    )?;
    // One address doing half the site's normal traffic is a burst; a network
    // needs the whole of it, a provider twice it, the site itself three times.
    // The floors stop quiet sites flagging a visitor loading one page.
    let (ip_rate, network_rate, asn_rate, site_rate) = match params.threshold {
        Some(rate) => (rate, rate * 2, rate * 5, rate * 5),
        None => (
            (median * 0.5).ceil().max(60.0) as u64,
            median.ceil().max(120.0) as u64,
            (median * 2.0).ceil().max(300.0) as u64,
            (median * 3.0).ceil().max(100.0) as u64,
        ),
    };

    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE loga_burst_minutes AS
           WITH networks AS (
             SELECT ip_net, any_value(provider) AS provider, m, sum(c) AS c, count(*) AS ips
             FROM loga_ip_minutes GROUP BY ip_net, m)
           SELECT 'address' AS level, ip AS target, m, c FROM loga_ip_minutes WHERE c >= {ip_rate}
           UNION ALL
           SELECT 'network', ip_net, m, c FROM networks WHERE c >= {network_rate} AND ips >= 2
           UNION ALL
           SELECT 'provider', provider, m, sum(c) FROM networks WHERE provider IS NOT NULL
           GROUP BY provider, m HAVING sum(c) >= {asn_rate} AND count(*) >= 2
           UNION ALL
           SELECT 'site', '(whole site)', m, sum(c) FROM loga_ip_minutes
           GROUP BY m HAVING sum(c) >= {site_rate};

         CREATE OR REPLACE TEMP TABLE loga_incidents AS
           WITH marked AS (
             SELECT *, CASE WHEN lag(m) OVER w IS NULL
                              OR m - lag(m) OVER w > INTERVAL {INCIDENT_GAP_MINUTES} MINUTE
                            THEN 1 ELSE 0 END AS starts
             FROM loga_burst_minutes
             WINDOW w AS (PARTITION BY level, target ORDER BY m)),
           grouped AS (
             SELECT *, sum(starts) OVER (PARTITION BY level, target ORDER BY m) AS g FROM marked),
           merged AS (
             SELECT level, target, min(m) AS start_ts, max(m) + INTERVAL 1 MINUTE AS end_ts,
                    count(*) AS burst_minutes, max(c) AS peak
             FROM grouped GROUP BY level, target, g)
           SELECT row_number() OVER (ORDER BY peak DESC, start_ts, level, target) AS incident_id, *
           FROM merged ORDER BY peak DESC, start_ts, level, target LIMIT {MAX_INCIDENTS};

         -- Every minute each incident covers, so requests can be matched to
         -- incidents by equality (fast) instead of by time range (slow).
         CREATE OR REPLACE TEMP TABLE loga_incident_minutes AS
           SELECT incident_id, level, target,
                  unnest(generate_series(start_ts, end_ts - INTERVAL 1 MINUTE, INTERVAL 1 MINUTE)) AS m
           FROM loga_incidents;

         CREATE OR REPLACE TEMP TABLE loga_incident_rows AS
           WITH r AS MATERIALIZED (
             SELECT r.ip, r.ip_net, 'AS' || CAST(r.asn AS VARCHAR) AS provider, r.path, r.raw_request,
                    r.status, r.ua_id, r.bot_verify, r.ts_local, r.asn_org,
                    date_trunc('minute', r.ts) AS m
             FROM requests r WHERE {filter})
           SELECT i.incident_id, r.ip, r.path, r.raw_request, r.status, r.ua_id, r.bot_verify,
                  r.ts_local, r.asn_org
           FROM r JOIN loga_incident_minutes i ON i.level = 'address' AND r.ip = i.target AND r.m = i.m
           UNION ALL
           SELECT i.incident_id, r.ip, r.path, r.raw_request, r.status, r.ua_id, r.bot_verify,
                  r.ts_local, r.asn_org
           FROM r JOIN loga_incident_minutes i ON i.level = 'network' AND r.ip_net = i.target AND r.m = i.m
           UNION ALL
           SELECT i.incident_id, r.ip, r.path, r.raw_request, r.status, r.ua_id, r.bot_verify,
                  r.ts_local, r.asn_org
           FROM r JOIN loga_incident_minutes i ON i.level = 'provider' AND r.provider = i.target AND r.m = i.m
           UNION ALL
           SELECT i.incident_id, r.ip, r.path, r.raw_request, r.status, r.ua_id, r.bot_verify,
                  r.ts_local, r.asn_org
           FROM r JOIN (SELECT * FROM loga_incident_minutes WHERE level = 'site') i ON r.m = i.m;"
    ))?;

    scope.notes.push(format!(
        "Normal for this site over the range is {median:.0} requests a minute. A minute counts as a burst at \
         {ip_rate} or more from one address, {network_rate} from one network (at least two addresses), \
         {asn_rate} from one provider (at least two networks), or {site_rate} across the whole site. \
         Bursts less than {INCIDENT_GAP_MINUTES} minutes apart are one incident."
    ));
    scope.notes.push(
        "Requests, addresses and errors cover the whole incident from its first burst minute to its last. \
         A verified bot or the server's own address can cause an incident; neither goes into a block list."
            .to_owned(),
    );
    scope.finish(
        "bursts",
        vec![
            TableSpec {
                name: "incidents",
                columns: vec![
                    col("incident", Integer),
                    col("level", Text),
                    col("target", Text),
                    col("network_owner", Text),
                    col("start", Datetime),
                    col("end", Datetime),
                    col("burst_minutes", Integer),
                    col("peak_per_minute", Integer),
                    col("requests", Integer),
                    col("ips", Integer),
                    col("errors", Integer),
                    col("blocked", Integer),
                    col("top_path", Text),
                    col("agent_type", Text),
                    col("agent", Text),
                    col("verify", Text),
                ],
                sql: "SELECT i.incident_id AS incident, i.level, i.target,
                             CASE WHEN i.level = 'site' THEN NULL ELSE mode(x.asn_org) END AS network_owner,
                             strftime(min(x.ts_local), '%Y-%m-%d %H:%M:%S') AS start,
                             strftime(max(x.ts_local), '%Y-%m-%d %H:%M:%S') AS \"end\",
                             i.burst_minutes, i.peak AS peak_per_minute,
                             count(*) AS requests, count(DISTINCT x.ip) AS ips,
                             count(*) FILTER (WHERE x.status >= 400) AS errors,
                             count(*) FILTER (WHERE x.status IN (403, 429, 444)) AS blocked,
                             mode(coalesce(x.path, x.raw_request)) AS top_path,
                             mode(u.category) AS agent_type, mode(u.family) AS agent,
                             CASE WHEN i.level = 'site' THEN NULL ELSE max(x.bot_verify) END AS verify
                      FROM loga_incidents i JOIN loga_incident_rows x USING (incident_id)
                      LEFT JOIN user_agents u ON u.ua_id = x.ua_id
                      GROUP BY i.incident_id, i.level, i.target, i.burst_minutes, i.peak"
                    .to_owned(),
                order_by: "peak_per_minute DESC, incident",
                totals: None,
            },
            TableSpec {
                name: "top_paths",
                columns: vec![col("incident", Integer), col("path", Text), col("hits", Integer)],
                sql: "SELECT incident, path, hits FROM (
                        SELECT incident_id AS incident, coalesce(path, raw_request) AS path, count(*) AS hits,
                               row_number() OVER (PARTITION BY incident_id ORDER BY count(*) DESC) AS rank
                        FROM loga_incident_rows GROUP BY 1, 2)
                      WHERE rank <= 5"
                    .to_owned(),
                order_by: "incident, hits DESC, path",
                totals: None,
            },
            TableSpec {
                name: "status_mix",
                columns: vec![col("incident", Integer), col("status", Integer), col("hits", Integer)],
                sql: "SELECT incident_id AS incident, status, count(*) AS hits
                      FROM loga_incident_rows GROUP BY 1, 2"
                    .to_owned(),
                order_by: "incident, hits DESC, status",
                totals: None,
            },
        ],
    )
}
