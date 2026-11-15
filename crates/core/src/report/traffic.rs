//! Traffic reports (spec 6.1): overview, sources, campaigns, landing pages, AI referrals.

use super::ColumnType::{Datetime, Integer, String as Text};
use super::{ColumnType, Granularity, ReportParams, ReportResult, Scope, TableSpec, col};
use crate::error::Result;
use crate::store::Store;

const PAGES_NOTE: &str = "Pages are estimated from request logs: GET requests from browser user agents \
                          that returned 200 or 304 and are not static assets.";

/// Extracts one raw query parameter and URL-decodes it. Empty when absent.
fn query_param(name: &str) -> String {
    format!("url_decode(replace(regexp_extract(r.query, '(?:^|&){name}=([^&]*)', 1), '+', ' '))")
}

const TOTAL_COLUMNS: [(&str, ColumnType); 10] = [
    ("requests", Integer),
    ("pages", Integer),
    ("bytes", Integer),
    ("unique_ips", Integer),
    ("human", Integer),
    ("bot", Integer),
    ("s2xx", Integer),
    ("s3xx", Integer),
    ("s4xx", Integer),
    ("s5xx", Integer),
];

/// The aggregate list shared by the overview series and totals, over raw rows.
const RAW_AGGREGATES: &str = "count(*) AS requests,
    count(*) FILTER (WHERE r.is_page) AS pages,
    coalesce(sum(r.bytes), 0) AS bytes,
    count(DISTINCT r.ip) AS unique_ips,
    count(*) FILTER (WHERE u.category = 'browser') AS human,
    count(*) FILTER (WHERE u.category IN ('bot', 'tool', 'library')) AS bot,
    count(*) FILTER (WHERE r.status BETWEEN 200 AND 299) AS s2xx,
    count(*) FILTER (WHERE r.status BETWEEN 300 AND 399) AS s3xx,
    count(*) FILTER (WHERE r.status BETWEEN 400 AND 499) AS s4xx,
    count(*) FILTER (WHERE r.status BETWEEN 500 AND 599) AS s5xx";

pub fn overview(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    let filter = scope.filter.clone();
    let granularity = params.granularity.unwrap_or(Granularity::Day);
    let bucket = match granularity {
        Granularity::Day => "CAST(r.ts_local AS DATE)",
        Granularity::Hour => "date_trunc('hour', r.ts_local)",
    };
    let raw_series = format!(
        "SELECT {bucket} AS bucket, {RAW_AGGREGATES}
         FROM requests r LEFT JOIN user_agents u USING (ua_id)
         WHERE {filter} GROUP BY 1"
    );
    let raw_totals = format!(
        "SELECT {RAW_AGGREGATES} FROM requests r LEFT JOIN user_agents u USING (ua_id) WHERE {filter}"
    );

    // Pruned days can be answered from `daily_totals`, but only per day and
    // without filters: the rollup has neither hours nor the filtered columns.
    let rollup = match scope.pruned_before {
        Some(cutoff) if scope.coverage.rollup_days > 0 => {
            if granularity == Granularity::Day && params.filters.is_empty() {
                Some(format!(
                    "SELECT day AS bucket, requests, pages, bytes, unique_ips, human, bot, s2xx, s3xx, s4xx, s5xx
                     FROM daily_totals
                     WHERE day >= DATE '{}' AND day <= DATE '{}' AND day < DATE '{cutoff}'",
                    scope.from, scope.to
                ))
            } else {
                scope.raw_only();
                None
            }
        }
        _ => None,
    };

    let (series_sql, totals_sql) = match rollup {
        Some(rollup) => {
            scope.notes.push(format!(
                "{} day(s) come from daily rollups because their raw rows were pruned. \
                 Unique IPs cannot be added across days, so the range total is null.",
                scope.coverage.rollup_days
            ));
            let sums = TOTAL_COLUMNS
                .iter()
                .map(|(key, _)| match *key {
                    "unique_ips" => "NULL AS unique_ips".to_owned(),
                    key => format!("coalesce(sum({key}), 0) AS {key}"),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let union = format!("{rollup} UNION ALL {raw_series}");
            let totals = format!("SELECT {sums} FROM ({union}) s");
            (union, totals)
        }
        None => (raw_series, raw_totals),
    };

    let value_columns = || TOTAL_COLUMNS.iter().map(|(key, ty)| col(key, *ty));
    let bucket_type = match granularity {
        Granularity::Day => ColumnType::Date,
        Granularity::Hour => Datetime,
    };
    scope.notes.push(PAGES_NOTE.to_owned());
    scope.notes.push(
        "Human means a browser user agent; bot means a declared bot, tool or HTTP library. \
         Empty and unrecognised user agents are in neither."
            .to_owned(),
    );
    scope.finish(
        "overview",
        vec![
            TableSpec {
                name: "series",
                columns: std::iter::once(col("bucket", bucket_type))
                    .chain(value_columns())
                    .collect(),
                sql: series_sql,
                order_by: "bucket",
                totals: None,
            },
            TableSpec {
                name: "totals",
                columns: value_columns().collect(),
                sql: totals_sql,
                order_by: "1",
                totals: None,
            },
        ],
    )
}

fn page_totals(filter: &str) -> Option<(Vec<super::Column>, String)> {
    Some((
        vec![col("pages", Integer), col("requests", Integer)],
        format!(
            "SELECT count(*) FILTER (WHERE r.is_page) AS pages, count(*) AS requests FROM requests r WHERE {filter}"
        ),
    ))
}

pub fn sources(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    let filter = scope.filter.clone();
    // Clicks within the site say nothing about where visitors came from.
    let external = format!("{filter} AND r.source_type <> 'internal' AND r.referer IS NOT NULL");
    scope.notes.push(PAGES_NOTE.to_owned());
    scope.notes.push(
        "Source is set per request, not per visit, so later pages of a visit count as internal. \
         The referrer tables leave internal referrers out."
            .to_owned(),
    );
    scope.finish(
        "sources",
        vec![
            TableSpec {
                name: "by_source_type",
                columns: vec![col("source_type", Text), col("pages", Integer), col("requests", Integer)],
                sql: format!(
                    "SELECT r.source_type, count(*) FILTER (WHERE r.is_page) AS pages, count(*) AS requests
                     FROM requests r WHERE {filter} GROUP BY 1"
                ),
                order_by: "pages DESC, requests DESC, source_type",
                totals: page_totals(&filter),
            },
            TableSpec {
                name: "by_referrer_host",
                columns: vec![
                    col("source_type", Text),
                    col("referer_host", Text),
                    col("pages", Integer),
                    col("requests", Integer),
                ],
                sql: format!(
                    "SELECT r.source_type, r.referer_host, count(*) FILTER (WHERE r.is_page) AS pages,
                            count(*) AS requests
                     FROM requests r WHERE {external} GROUP BY 1, 2"
                ),
                order_by: "pages DESC, requests DESC, referer_host",
                totals: page_totals(&external),
            },
            TableSpec {
                name: "by_referrer_url",
                columns: vec![
                    col("source_type", Text),
                    col("referer", Text),
                    col("pages", Integer),
                    col("requests", Integer),
                ],
                sql: format!(
                    "SELECT r.source_type, r.referer, count(*) FILTER (WHERE r.is_page) AS pages,
                            count(*) AS requests
                     FROM requests r WHERE {external} GROUP BY 1, 2"
                ),
                order_by: "pages DESC, requests DESC, referer",
                totals: page_totals(&external),
            },
        ],
    )
}

pub fn campaigns(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    let filter = scope.filter.clone();
    let tagged = format!("{filter} AND r.query IS NOT NULL AND contains(r.query, 'utm_')");
    let (source, medium, campaign) = (
        query_param("utm_source"),
        query_param("utm_medium"),
        query_param("utm_campaign"),
    );
    let click_ids = [
        ("gclid", "Google Ads"),
        ("gbraid", "Google Ads (iOS app)"),
        ("wbraid", "Google Ads (iOS web)"),
        ("dclid", "Google Display"),
        ("fbclid", "Meta"),
        ("msclkid", "Microsoft Ads"),
        ("ttclid", "TikTok"),
        ("twclid", "X"),
        ("li_fat_id", "LinkedIn"),
    ];
    let click_id_sql = click_ids
        .iter()
        .map(|(param, network)| {
            format!(
                "SELECT '{param}' AS click_id, '{network}' AS network,
                        count(*) FILTER (WHERE r.is_page) AS pages, count(*) AS requests
                 FROM requests r
                 WHERE {filter} AND r.query IS NOT NULL
                   AND regexp_matches(r.query, '(?:^|&){param}=')"
            )
        })
        .collect::<Vec<_>>()
        .join(" UNION ALL ");

    scope.notes.push(PAGES_NOTE.to_owned());
    scope.finish(
        "campaigns",
        vec![
            TableSpec {
                name: "campaigns",
                columns: vec![
                    col("utm_source", Text),
                    col("utm_medium", Text),
                    col("utm_campaign", Text),
                    col("pages", Integer),
                    col("requests", Integer),
                ],
                sql: format!(
                    "SELECT * FROM (
                       SELECT nullif({source}, '') AS utm_source, nullif({medium}, '') AS utm_medium,
                              nullif({campaign}, '') AS utm_campaign,
                              count(*) FILTER (WHERE r.is_page) AS pages, count(*) AS requests
                       FROM requests r WHERE {tagged} GROUP BY 1, 2, 3)
                     WHERE utm_source IS NOT NULL OR utm_medium IS NOT NULL OR utm_campaign IS NOT NULL"
                ),
                order_by: "pages DESC, requests DESC, utm_source, utm_medium, utm_campaign",
                totals: None,
            },
            TableSpec {
                name: "click_ids",
                columns: vec![
                    col("click_id", Text),
                    col("network", Text),
                    col("pages", Integer),
                    col("requests", Integer),
                ],
                sql: format!("SELECT * FROM ({click_id_sql}) WHERE requests > 0"),
                order_by: "pages DESC, requests DESC, click_id",
                totals: None,
            },
        ],
    )
}

/// Page views that start a session: the first for an IP and user agent, or
/// the first after a gap of 30 minutes or more.
fn session_entries(filter: &str) -> String {
    format!(
        "SELECT path, source_type FROM (
           SELECT r.path, r.source_type, r.ts,
                  lag(r.ts) OVER (PARTITION BY r.ip, r.ua_id ORDER BY r.ts) AS previous
           FROM requests r WHERE {filter} AND r.is_page)
         WHERE previous IS NULL OR ts - previous >= INTERVAL 30 MINUTE"
    )
}

pub fn landing_pages(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    // The session window is the expensive part, and three queries need its
    // result, so compute it once. Temporary tables work on read-only databases.
    scope.conn().execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE loga_session_entries AS {}",
        session_entries(&scope.filter)
    ))?;
    let entries = "SELECT path, source_type FROM loga_session_entries";
    let totals = || {
        Some((
            vec![col("sessions", Integer)],
            format!("SELECT count(*) AS sessions FROM ({entries}) e"),
        ))
    };
    scope.notes.push(
        "Sessions are approximate: page views from the same IP address and user agent with gaps \
         under 30 minutes. Visitors behind a shared address are merged."
            .to_owned(),
    );
    scope.notes.push(PAGES_NOTE.to_owned());
    scope.finish(
        "landing-pages",
        vec![
            TableSpec {
                name: "landing_pages",
                columns: vec![col("path", Text), col("sessions", Integer)],
                sql: format!("SELECT path, count(*) AS sessions FROM ({entries}) e GROUP BY 1"),
                order_by: "sessions DESC, path",
                totals: totals(),
            },
            TableSpec {
                name: "by_source_type",
                columns: vec![col("source_type", Text), col("sessions", Integer)],
                sql: format!(
                    "SELECT source_type, count(*) AS sessions FROM ({entries}) e GROUP BY 1"
                ),
                order_by: "sessions DESC, source_type",
                totals: totals(),
            },
        ],
    )
}

pub fn ai_referrals(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    let filter = format!("{} AND r.source_type = 'ai'", scope.filter);
    // Assistants that strip the referrer are identified by their utm_source tag.
    let assistant = format!(
        "coalesce(r.referer_host, nullif(lower({}), ''), '(unknown)')",
        query_param("utm_source")
    );
    scope.notes.push(PAGES_NOTE.to_owned());
    scope.notes.push(
        "Only visits that arrive with an assistant's referrer or utm_source tag are counted. \
         Assistants that send neither appear as direct traffic."
            .to_owned(),
    );
    scope.finish(
        "ai-referrals",
        vec![
            TableSpec {
                name: "by_assistant",
                columns: vec![
                    col("assistant", Text),
                    col("pages", Integer),
                    col("requests", Integer),
                ],
                sql: format!(
                    "SELECT {assistant} AS assistant, count(*) FILTER (WHERE r.is_page) AS pages,
                            count(*) AS requests
                     FROM requests r WHERE {filter} GROUP BY 1"
                ),
                order_by: "pages DESC, requests DESC, assistant",
                totals: page_totals(&filter),
            },
            TableSpec {
                name: "by_landing_page",
                columns: vec![
                    col("assistant", Text),
                    col("path", Text),
                    col("pages", Integer),
                ],
                sql: format!(
                    "SELECT {assistant} AS assistant, r.path, count(*) AS pages
                     FROM requests r WHERE {filter} AND r.is_page GROUP BY 1, 2"
                ),
                order_by: "pages DESC, assistant, path",
                totals: None,
            },
        ],
    )
}

/// Where traffic comes from: countries and networks. The network view is the
/// one for spotting hosting-provider traffic posing as visitors.
pub fn geography(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    let filter = scope.filter.clone();
    scope.notes.push(PAGES_NOTE.to_owned());
    scope.notes.push(
        "Country and network come from the geo database in Settings, and are approximate. Requests \
         ingested before one was installed show as unknown until bots are re-checked on the Storage screen."
            .to_owned(),
    );
    scope.finish(
        "geography",
        vec![
            TableSpec {
                name: "by_country",
                columns: vec![
                    col("country", Text),
                    col("ips", Integer),
                    col("pages", Integer),
                    col("requests", Integer),
                    col("human", Integer),
                    col("bot", Integer),
                ],
                sql: format!(
                    "SELECT coalesce(r.country, '(unknown)') AS country, count(DISTINCT r.ip) AS ips,
                            count(*) FILTER (WHERE r.is_page) AS pages, count(*) AS requests,
                            count(*) FILTER (WHERE u.category = 'browser') AS human,
                            count(*) FILTER (WHERE u.category IN ('bot', 'tool', 'library')) AS bot
                     FROM requests r LEFT JOIN user_agents u USING (ua_id) WHERE {filter} GROUP BY 1"
                ),
                order_by: "requests DESC, country",
                totals: page_totals(&filter),
            },
            TableSpec {
                name: "by_asn",
                columns: vec![
                    col("asn", Integer),
                    col("asn_org", Text),
                    col("ips", Integer),
                    col("pages", Integer),
                    col("requests", Integer),
                    col("human", Integer),
                    col("bot", Integer),
                ],
                sql: format!(
                    "SELECT r.asn, coalesce(any_value(r.asn_org), '(unknown)') AS asn_org,
                            count(DISTINCT r.ip) AS ips,
                            count(*) FILTER (WHERE r.is_page) AS pages, count(*) AS requests,
                            count(*) FILTER (WHERE u.category = 'browser') AS human,
                            count(*) FILTER (WHERE u.category IN ('bot', 'tool', 'library')) AS bot
                     FROM requests r LEFT JOIN user_agents u USING (ua_id) WHERE {filter} GROUP BY r.asn"
                ),
                order_by: "requests DESC, asn",
                totals: None,
            },
        ],
    )
}
