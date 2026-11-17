//! SEO crawl reports (spec 6.4): what search engine crawlers fetched, how
//! often, and what the site answered.
//!
//! Every report here counts verified search engine crawlers unless the bot or
//! verified filter says otherwise. A URL is the path plus the query string,
//! shown as two columns.

use super::ColumnType::{Datetime, Float, Integer, String as Text};
use super::bots::{BOT_ROWS, SEEN, load_registry};
use super::{ColumnType, Granularity, ReportParams, ReportResult, Scope, TableSpec, col, quote};
use crate::classify::source::SourceRules;
use crate::error::Result;
use crate::store::Store;

/// 304 is left out: it answers a conditional request and points nowhere.
const REDIRECT: &str = "r.status IN (301, 302, 303, 307, 308)";

const URLS: &str = "count(DISTINCT concat(r.path, '?', r.query)) AS urls";

const HOUSEKEEPING: &str = "(r.path = '/robots.txt'
     OR regexp_matches(lower(r.path), 'sitemap[^/]*\\.(xml|xml\\.gz|txt)$'))";

/// Whose requests a report counts when no bot filter is set.
#[derive(Clone, Copy)]
enum Subject {
    SearchBots,
    Googlebot,
}

/// The shared scope plus the condition that narrows `BOT_ROWS` to the
/// crawlers the report is about.
fn crawl_scope<'a>(
    store: &'a Store,
    params: &'a ReportParams,
    subject: Subject,
) -> Result<(Scope<'a>, String)> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    load_registry(&scope, store)?;
    let filters = &params.filters;
    let (who, mut filter) = match (subject, &filters.bot) {
        (Subject::Googlebot, _) => (
            "Googlebot",
            format!("{} AND u.bot_key = 'googlebot'", scope.filter),
        ),
        (Subject::SearchBots, Some(_)) => ("the chosen bot", scope.filter.clone()),
        (Subject::SearchBots, None) => (
            "search engine crawlers",
            format!("{} AND b.kind = 'search'", scope.filter),
        ),
    };
    filter.push_str(" AND r.path IS NOT NULL");

    match &filters.verified {
        Some(state) => scope.notes.push(format!(
            "This report counts requests from {who} whose verified state is {}.",
            state.to_ascii_lowercase()
        )),
        None => {
            let (unverifiable, fake) = scope.conn().query_row(
                &format!(
                    "SELECT count(*) FILTER (WHERE r.bot_verify = 'unverifiable'),
                            count(*) FILTER (WHERE r.bot_verify = 'fake')
                     FROM {BOT_ROWS} WHERE {filter}"
                ),
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?;
            filter.push_str(" AND r.bot_verify = 'verified'");
            scope.notes.push(match (subject, &filters.bot) {
                (Subject::SearchBots, None) => {
                    "This report counts requests from search engine crawlers (Googlebot, Bingbot and \
                     the like) whose address was verified. Filter by bot to look at one crawler, or at \
                     a bot of another kind."
                        .to_owned()
                }
                _ => format!(
                    "This report counts requests from {who} whose address was verified."
                ),
            });
            if unverifiable > 0 {
                scope.notes.push(format!(
                    "{unverifiable} request(s) from {who} could not be verified and are left out. That \
                     happens when the operator publishes no usable address list, or none is cached (run \
                     `logloupe ranges update`). Filter by the verified state `unverifiable` to see them."
                ));
            }
            if fake > 0 {
                scope.notes.push(format!(
                    "{fake} request(s) claiming to be {who} came from outside the published address \
                     ranges and are left out. The fake-bots report lists them."
                ));
            }
        }
    }
    Ok((scope, filter))
}

fn bucket(params: &ReportParams) -> (&'static str, ColumnType) {
    match params.granularity {
        Some(Granularity::Hour) => ("date_trunc('hour', r.ts_local)", Datetime),
        _ => ("CAST(r.ts_local AS DATE)", ColumnType::Date),
    }
}

/// True for static assets and internal endpoints, by the same rules ingest
/// uses to tell pages from everything else.
fn asset_condition() -> String {
    let rules = SourceRules::with_defaults(&[]);
    let mut clauses: Vec<String> = rules
        .non_page_prefixes()
        .iter()
        .map(|prefix| format!("starts_with(r.path, {})", quote(prefix)))
        .collect();
    let extensions: Vec<String> = rules
        .asset_extensions()
        .iter()
        .map(|ext| quote(ext))
        .collect();
    if !extensions.is_empty() {
        clauses.push(format!(
            "lower(regexp_extract(r.path, '\\.([^./]*)$', 1)) IN ({})",
            extensions.join(", ")
        ));
    }
    if clauses.is_empty() {
        "false".to_owned()
    } else {
        format!("({})", clauses.join(" OR "))
    }
}

pub fn crawl(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let (scope, filter) = crawl_scope(store, params, Subject::SearchBots)?;
    let (bucket, bucket_type) = bucket(params);
    scope.finish(
        "crawl",
        vec![
            TableSpec {
                name: "by_bot",
                columns: vec![
                    col("bot", Text),
                    col("operator", Text),
                    col("hits", Integer),
                    col("urls", Integer),
                    col("bytes", Integer),
                    col("redirects", Integer),
                    col("errors", Integer),
                    col("first_seen", Datetime),
                    col("last_seen", Datetime),
                ],
                sql: format!(
                    "SELECT b.bot, b.operator, count(*) AS hits, {URLS},
                            coalesce(sum(r.bytes), 0) AS bytes,
                            count(*) FILTER (WHERE {REDIRECT}) AS redirects,
                            count(*) FILTER (WHERE r.status >= 400) AS errors, {SEEN}
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1, 2"
                ),
                order_by: "hits DESC, bot",
                totals: Some((
                    vec![
                        col("hits", Integer),
                        col("urls", Integer),
                        col("bytes", Integer),
                    ],
                    format!(
                        "SELECT count(*) AS hits, {URLS}, coalesce(sum(r.bytes), 0) AS bytes
                         FROM {BOT_ROWS} WHERE {filter}"
                    ),
                )),
            },
            TableSpec {
                name: "series",
                columns: vec![
                    col("bucket", bucket_type),
                    col("bot", Text),
                    col("hits", Integer),
                ],
                sql: format!(
                    "SELECT {bucket} AS bucket, b.bot, count(*) AS hits
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1, 2"
                ),
                order_by: "bucket, hits DESC, bot",
                totals: None,
            },
            TableSpec {
                name: "directories",
                columns: vec![
                    col("directory", Text),
                    col("hits", Integer),
                    col("share", Float),
                    col("urls", Integer),
                    col("bytes", Integer),
                    col("errors", Integer),
                    col("last_seen", Datetime),
                ],
                sql: format!(
                    "SELECT CASE WHEN r.path LIKE '/%/%' THEN '/' || split_part(r.path, '/', 2) || '/'
                                 ELSE '/' END AS directory,
                            count(*) AS hits,
                            round(100.0 * count(*) / sum(count(*)) OVER (), 1) AS share,
                            {URLS}, coalesce(sum(r.bytes), 0) AS bytes,
                            count(*) FILTER (WHERE r.status >= 400) AS errors,
                            strftime(max(r.ts_local), '%Y-%m-%d %H:%M:%S') AS last_seen
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1"
                ),
                order_by: "hits DESC, directory",
                totals: None,
            },
        ],
    )
}

pub fn crawled_urls(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let (mut scope, filter) = crawl_scope(store, params, Subject::SearchBots)?;
    let timed: bool = scope.conn().query_row(
        &format!("SELECT count(*) > 0 FROM {BOT_ROWS} WHERE {filter} AND r.request_ms IS NOT NULL"),
        [],
        |row| row.get(0),
    )?;

    let mut url_columns = vec![
        col("path", Text),
        col("query", Text),
        col("hits", Integer),
        col("first_seen", Datetime),
        col("last_seen", Datetime),
        col("last_status", Integer),
        col("avg_bytes", Integer),
    ];
    if timed {
        url_columns.push(col("avg_ms", Integer));
    }
    let urls_sql = format!(
        "SELECT r.path, r.query, count(*) AS hits, {SEEN},
                arg_max(r.status, r.ts) AS last_status,
                round(avg(r.bytes) FILTER (WHERE r.status <> 304)) AS avg_bytes,
                round(avg(r.request_ms)) AS avg_ms
         FROM {BOT_ROWS} WHERE {filter} GROUP BY 1, 2"
    );
    let mut specs = vec![
        TableSpec {
            name: "urls",
            columns: url_columns.clone(),
            sql: urls_sql.clone(),
            order_by: "hits DESC, path, query",
            totals: Some((
                vec![col("hits", Integer), col("urls", Integer)],
                format!("SELECT count(*) AS hits, {URLS} FROM {BOT_ROWS} WHERE {filter}"),
            )),
        },
        TableSpec {
            name: "least_crawled",
            columns: url_columns,
            sql: urls_sql,
            order_by: "hits, last_seen, path, query",
            totals: None,
        },
        TableSpec {
            name: "largest",
            columns: vec![
                col("path", Text),
                col("query", Text),
                col("hits", Integer),
                col("avg_bytes", Integer),
                col("max_bytes", Integer),
                col("total_bytes", Integer),
            ],
            sql: format!(
                "SELECT r.path, r.query, count(*) AS hits, round(avg(r.bytes)) AS avg_bytes,
                        max(r.bytes) AS max_bytes, sum(r.bytes) AS total_bytes
                 FROM {BOT_ROWS} WHERE {filter} AND r.status <> 304 AND r.bytes > 0 GROUP BY 1, 2"
            ),
            order_by: "avg_bytes DESC, path, query",
            totals: None,
        },
    ];
    scope.notes.push(
        "A URL is the path plus its query string. The least crawled table lists the same URLs from the \
         other end: fewest fetches first, then those fetched longest ago. Average bytes leaves out 304 \
         responses, which have no body."
            .to_owned(),
    );
    if timed {
        specs.push(TableSpec {
            name: "slowest",
            columns: vec![
                col("path", Text),
                col("query", Text),
                col("hits", Integer),
                col("avg_ms", Integer),
                col("max_ms", Integer),
            ],
            sql: format!(
                "SELECT r.path, r.query, count(*) AS hits, round(avg(r.request_ms)) AS avg_ms,
                        max(r.request_ms) AS max_ms
                 FROM {BOT_ROWS} WHERE {filter} AND r.request_ms IS NOT NULL GROUP BY 1, 2"
            ),
            order_by: "avg_ms DESC, path, query",
            totals: None,
        });
    } else {
        scope.notes.push(
            "These logs do not record how long each response took, so there are no response times and \
             no slowest URLs table."
                .to_owned(),
        );
    }
    scope.finish("crawled-urls", specs)
}

pub fn crawl_errors(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let (mut scope, filter) = crawl_scope(store, params, Subject::SearchBots)?;
    let by_status = |name: &'static str, condition: &str| {
        TableSpec {
        name,
        columns: vec![
            col("path", Text),
            col("query", Text),
            col("status", Integer),
            col("hits", Integer),
            col("bots", Integer),
            col("first_seen", Datetime),
            col("last_seen", Datetime),
        ],
        sql: format!(
            "SELECT r.path, r.query, r.status, count(*) AS hits, count(DISTINCT b.bot) AS bots, {SEEN}
             FROM {BOT_ROWS} WHERE {filter} AND {condition} GROUP BY 1, 2, 3"
        ),
        order_by: "hits DESC, path, query, status",
        totals: Some((
            vec![col("hits", Integer), col("urls", Integer)],
            format!(
                "SELECT count(*) AS hits, {URLS} FROM {BOT_ROWS} WHERE {filter} AND {condition}"
            ),
        )),
    }
    };
    scope.notes.push(
        "Errors are responses with status 400 or above. Redirects are 301, 302, 303, 307 and 308; the \
         log does not record where a redirect points. A URL is inconsistent when it answered with more \
         than one status class in the period (a 304 counts as a success)."
            .to_owned(),
    );
    scope.finish(
        "crawl-errors",
        vec![
            by_status("errors", "r.status >= 400"),
            by_status("redirects", REDIRECT),
            TableSpec {
                name: "inconsistent",
                columns: vec![
                    col("path", Text),
                    col("query", Text),
                    col("statuses", Text),
                    col("hits", Integer),
                    col("last_status", Integer),
                    col("first_seen", Datetime),
                    col("last_seen", Datetime),
                ],
                sql: format!(
                    "SELECT r.path, r.query,
                            array_to_string(list_sort(list_distinct(list(CAST(r.status AS VARCHAR)))), ', ')
                              AS statuses,
                            count(*) AS hits, arg_max(r.status, r.ts) AS last_status, {SEEN}
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1, 2
                     HAVING count(DISTINCT CASE WHEN r.status = 304 THEN 2
                                                ELSE CAST(floor(r.status / 100) AS INTEGER) END) > 1"
                ),
                order_by: "hits DESC, path, query",
                totals: None,
            },
        ],
    )
}

pub fn crawl_waste(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let (mut scope, filter) = crawl_scope(store, params, Subject::SearchBots)?;
    let category = format!(
        "CASE WHEN r.status >= 400 THEN 'error'
              WHEN {REDIRECT} THEN 'redirect'
              WHEN {HOUSEKEEPING} THEN 'housekeeping'
              WHEN {} THEN 'asset'
              WHEN r.query IS NOT NULL THEN 'parameters'
              ELSE 'page' END",
        asset_condition()
    );
    // Digits and query values are replaced so that URLs differing only in
    // an ID, a page number or a value group together.
    let pattern = "regexp_replace(r.path, '[0-9]+', '#', 'g') || CASE WHEN r.query IS NULL THEN ''
         ELSE '?' || left(regexp_replace(r.query, '=[^&?]*', '=…', 'g'), 200) END";
    scope.notes.push(
        "Each request is counted once, in the first category it fits: error (status 400 or above), \
         redirect, housekeeping (robots.txt and sitemaps), asset, parameters (a URL with a query \
         string), then page. Errors, redirects and parameterised URLs are the usual crawl waste. Assets \
         are not waste in themselves: a search engine needs the style sheets and scripts to render a page."
            .to_owned(),
    );
    scope.notes.push(
        "A pattern is the URL with every run of digits replaced by # and every query value by …, so \
         URLs that differ only in an ID, a page number or a value group together."
            .to_owned(),
    );
    scope.finish(
        "crawl-waste",
        vec![
            TableSpec {
                name: "summary",
                columns: vec![
                    col("category", Text),
                    col("hits", Integer),
                    col("share", Float),
                    col("urls", Integer),
                    col("bytes", Integer),
                ],
                sql: format!(
                    "SELECT {category} AS category, count(*) AS hits,
                            round(100.0 * count(*) / sum(count(*)) OVER (), 1) AS share,
                            {URLS}, coalesce(sum(r.bytes), 0) AS bytes
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1"
                ),
                order_by: "hits DESC, category",
                totals: Some((
                    vec![col("hits", Integer), col("wasted", Integer)],
                    format!(
                        "SELECT count(*) AS hits,
                                count(*) FILTER (WHERE category IN ('error', 'redirect', 'parameters')) AS wasted
                         FROM (SELECT {category} AS category FROM {BOT_ROWS} WHERE {filter})"
                    ),
                )),
            },
            TableSpec {
                name: "patterns",
                columns: vec![
                    col("category", Text),
                    col("pattern", Text),
                    col("hits", Integer),
                    col("urls", Integer),
                    col("bytes", Integer),
                ],
                sql: format!(
                    "SELECT category, pattern, count(*) AS hits, count(DISTINCT url) AS urls,
                            coalesce(sum(bytes), 0) AS bytes
                     FROM (SELECT {category} AS category, {pattern} AS pattern,
                                  concat(r.path, '?', r.query) AS url, r.bytes
                           FROM {BOT_ROWS} WHERE {filter})
                     WHERE category IN ('error', 'redirect', 'parameters') GROUP BY 1, 2"
                ),
                order_by: "hits DESC, pattern, category",
                totals: None,
            },
        ],
    )
}

pub fn crawl_housekeeping(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let (mut scope, filter) = crawl_scope(store, params, Subject::SearchBots)?;
    let filter = format!("{filter} AND {HOUSEKEEPING}");
    scope.notes.push(
        "Fetches of /robots.txt and of sitemap files (a file name containing `sitemap` and ending in \
         .xml, .xml.gz or .txt). Google's documentation says it treats a robots.txt that answers with a \
         server error (5xx) as blocking the whole site, and one that answers 404 as allowing everything."
            .to_owned(),
    );
    scope.finish(
        "crawl-housekeeping",
        vec![
            TableSpec {
                name: "by_bot",
                columns: vec![
                    col("bot", Text),
                    col("file", Text),
                    col("hits", Integer),
                    col("ok", Integer),
                    col("errors", Integer),
                    col("last_status", Integer),
                    col("first_seen", Datetime),
                    col("last_seen", Datetime),
                ],
                sql: format!(
                    "SELECT b.bot, CASE WHEN r.path = '/robots.txt' THEN 'robots.txt' ELSE 'sitemap' END AS file,
                            count(*) AS hits, count(*) FILTER (WHERE r.status IN (200, 304)) AS ok,
                            count(*) FILTER (WHERE r.status >= 400) AS errors,
                            arg_max(r.status, r.ts) AS last_status, {SEEN}
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1, 2"
                ),
                order_by: "bot, file",
                totals: None,
            },
            TableSpec {
                name: "files",
                columns: vec![
                    col("path", Text),
                    col("bot", Text),
                    col("status", Integer),
                    col("hits", Integer),
                    col("last_seen", Datetime),
                ],
                sql: format!(
                    "SELECT r.path, b.bot, r.status, count(*) AS hits,
                            strftime(max(r.ts_local), '%Y-%m-%d %H:%M:%S') AS last_seen
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1, 2, 3"
                ),
                order_by: "hits DESC, path, bot, status",
                totals: None,
            },
        ],
    )
}

pub fn googlebot(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let (mut scope, filter) = crawl_scope(store, params, Subject::Googlebot)?;
    let (bucket, bucket_type) = bucket(params);
    let crawler = "CASE WHEN contains(lower(u.ua), 'googlebot-image') THEN 'Googlebot Image'
              WHEN contains(lower(u.ua), 'googlebot-video') THEN 'Googlebot Video'
              WHEN contains(lower(u.ua), 'googlebot-news') THEN 'Googlebot News'
              WHEN contains(lower(u.ua), 'android') OR contains(lower(u.ua), 'mobile')
                THEN 'Googlebot Smartphone'
              ELSE 'Googlebot Desktop' END";
    scope.notes.push(
        "Which Googlebot fetched the site, told apart by user agent: the smartphone crawler names a \
         phone, the desktop crawler does not. Google indexes from the smartphone crawler, so it should \
         be doing most of the crawling."
            .to_owned(),
    );
    scope.finish(
        "googlebot",
        vec![
            TableSpec {
                name: "by_crawler",
                columns: vec![
                    col("crawler", Text),
                    col("hits", Integer),
                    col("share", Float),
                    col("urls", Integer),
                    col("bytes", Integer),
                    col("errors", Integer),
                ],
                sql: format!(
                    "SELECT {crawler} AS crawler, count(*) AS hits,
                            round(100.0 * count(*) / sum(count(*)) OVER (), 1) AS share,
                            {URLS}, coalesce(sum(r.bytes), 0) AS bytes,
                            count(*) FILTER (WHERE r.status >= 400) AS errors
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1"
                ),
                order_by: "hits DESC, crawler",
                totals: Some((
                    vec![col("hits", Integer)],
                    format!("SELECT count(*) AS hits FROM {BOT_ROWS} WHERE {filter}"),
                )),
            },
            TableSpec {
                name: "series",
                columns: vec![
                    col("bucket", bucket_type),
                    col("crawler", Text),
                    col("hits", Integer),
                ],
                sql: format!(
                    "SELECT {bucket} AS bucket, {crawler} AS crawler, count(*) AS hits
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1, 2"
                ),
                order_by: "bucket, hits DESC, crawler",
                totals: None,
            },
        ],
    )
}
