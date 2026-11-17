//! Bot reports (spec 6.2): overview, fake bots, bot IPs, AI crawlers,
//! undeclared automation.

use super::ColumnType::{Datetime, Integer, String as Text};
use super::{ColumnType, ReportParams, ReportResult, Scope, TableSpec, col, quote};
use crate::bots::Registry;
use crate::error::Result;
use crate::store::Store;

const VERIFY_NOTE: &str = "A bot is verified when its address is inside the operator's published IP ranges, \
     fake when the operator publishes ranges and the address is outside them, and unverifiable when \
     the operator publishes no usable list or no ranges are cached (run `logloupe ranges update`).";

pub(super) const SEEN: &str = "strftime(min(r.ts_local), '%Y-%m-%d %H:%M:%S') AS first_seen,
                    strftime(max(r.ts_local), '%Y-%m-%d %H:%M:%S') AS last_seen";

const VERDICTS: &str = "count(*) FILTER (WHERE r.bot_verify = 'verified') AS verified,
                        count(*) FILTER (WHERE r.bot_verify = 'fake') AS fake,
                        count(*) FILTER (WHERE r.bot_verify = 'unverifiable') AS unverifiable";

/// Requests joined to their user agent and the registry's bot details.
pub(super) const BOT_ROWS: &str =
    "requests r JOIN user_agents u USING (ua_id) JOIN loga_bots b ON b.bot_key = u.bot_key";

/// Makes the registry available to SQL as the temporary table `loga_bots`.
pub(super) fn load_registry(scope: &Scope<'_>, store: &Store) -> Result<()> {
    let registry = Registry::load(store.data_root())?;
    let values: Vec<String> = registry
        .bots()
        .map(|(operator, bot)| {
            format!(
                "({}, {}, {}, {})",
                quote(&bot.key),
                quote(&bot.name),
                quote(&operator.name),
                quote(&bot.kind)
            )
        })
        .collect();
    scope.conn().execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE loga_bots AS
         SELECT * FROM (VALUES {}) t(bot_key, bot, operator, kind)",
        values.join(", ")
    ))?;
    Ok(())
}

fn verdict_columns() -> [super::Column; 3] {
    [
        col("verified", Integer),
        col("fake", Integer),
        col("unverifiable", Integer),
    ]
}

pub fn bots(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    load_registry(&scope, store)?;
    let filter = scope.filter.clone();
    let (bucket, bucket_type) = match params.granularity {
        Some(super::Granularity::Hour) => ("date_trunc('hour', r.ts_local)", Datetime),
        _ => ("CAST(r.ts_local AS DATE)", ColumnType::Date),
    };
    let mut by_bot = vec![
        col("bot", Text),
        col("operator", Text),
        col("kind", Text),
        col("hits", Integer),
        col("bytes", Integer),
        col("urls", Integer),
        col("errors", Integer),
    ];
    by_bot.extend(verdict_columns());
    scope.notes.push(VERIFY_NOTE.to_owned());
    scope
        .notes
        .push("Errors are responses with status 400 or above.".to_owned());
    scope.finish(
        "bots",
        vec![
            TableSpec {
                name: "by_bot",
                columns: by_bot,
                sql: format!(
                    "SELECT b.bot, b.operator, b.kind, count(*) AS hits, coalesce(sum(r.bytes), 0) AS bytes,
                            count(DISTINCT r.path) AS urls, count(*) FILTER (WHERE r.status >= 400) AS errors,
                            {VERDICTS}
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1, 2, 3"
                ),
                order_by: "hits DESC, bot",
                totals: Some((
                    {
                        let mut columns = vec![col("hits", Integer), col("bytes", Integer)];
                        columns.extend(verdict_columns());
                        columns
                    },
                    format!(
                        "SELECT count(*) AS hits, coalesce(sum(r.bytes), 0) AS bytes, {VERDICTS}
                         FROM {BOT_ROWS} WHERE {filter}"
                    ),
                )),
            },
            TableSpec {
                name: "by_kind",
                columns: vec![col("kind", Text), col("bots", Integer), col("hits", Integer), col("bytes", Integer)],
                sql: format!(
                    "SELECT b.kind, count(DISTINCT b.bot) AS bots, count(*) AS hits,
                            coalesce(sum(r.bytes), 0) AS bytes
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1"
                ),
                order_by: "hits DESC, kind",
                totals: None,
            },
            TableSpec {
                name: "series",
                columns: vec![col("bucket", bucket_type), col("kind", Text), col("hits", Integer)],
                sql: format!(
                    "SELECT {bucket} AS bucket, b.kind, count(*) AS hits
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1, 2"
                ),
                order_by: "bucket, hits DESC, kind",
                totals: None,
            },
        ],
    )
}

pub fn fake_bots(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    load_registry(&scope, store)?;
    let filter = format!("{} AND r.bot_verify = 'fake'", scope.filter);
    scope.notes.push(
        "These requests used a registered bot's user agent from an address outside that operator's \
         published IP ranges."
            .to_owned(),
    );
    scope.finish(
        "fake-bots",
        vec![
            TableSpec {
                name: "by_bot",
                columns: vec![
                    col("claimed_bot", Text),
                    col("hits", Integer),
                    col("ips", Integer),
                    col("first_seen", Datetime),
                    col("last_seen", Datetime),
                ],
                sql: format!(
                    "SELECT b.bot AS claimed_bot, count(*) AS hits, count(DISTINCT r.ip) AS ips, {SEEN}
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1"
                ),
                order_by: "hits DESC, claimed_bot",
                totals: Some((
                    vec![col("hits", Integer), col("ips", Integer)],
                    format!("SELECT count(*) AS hits, count(DISTINCT r.ip) AS ips FROM {BOT_ROWS} WHERE {filter}"),
                )),
            },
            TableSpec {
                name: "by_ip",
                columns: vec![
                    col("claimed_bot", Text),
                    col("ip", Text),
                    col("network", Text),
                    col("country", Text),
                    col("asn", Integer),
                    col("hits", Integer),
                    col("urls", Integer),
                    col("first_seen", Datetime),
                    col("last_seen", Datetime),
                ],
                sql: format!(
                    "SELECT b.bot AS claimed_bot, r.ip, r.ip_net AS network, any_value(r.country) AS country,
                            any_value(r.asn) AS asn, count(*) AS hits, count(DISTINCT r.path) AS urls, {SEEN}
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1, 2, 3"
                ),
                order_by: "hits DESC, ip",
                totals: None,
            },
            TableSpec {
                name: "paths",
                columns: vec![col("claimed_bot", Text), col("path", Text), col("hits", Integer)],
                sql: format!(
                    "SELECT b.bot AS claimed_bot, coalesce(r.path, r.raw_request) AS path, count(*) AS hits
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1, 2"
                ),
                order_by: "hits DESC, path",
                totals: None,
            },
        ],
    )
}

pub fn bot_ips(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    load_registry(&scope, store)?;
    let filter = scope.filter.clone();
    if params.filters.bot.is_none() {
        scope
            .notes
            .push("Showing every registered bot. Add --bot <name> to look at one.".to_owned());
    }
    scope.notes.push(VERIFY_NOTE.to_owned());
    scope.finish(
        "bot-ips",
        vec![TableSpec {
            name: "by_ip",
            columns: vec![
                col("bot", Text),
                col("ip", Text),
                col("verify", Text),
                col("hits", Integer),
                col("bytes", Integer),
                col("errors", Integer),
                col("first_seen", Datetime),
                col("last_seen", Datetime),
            ],
            sql: format!(
                "SELECT b.bot, r.ip, r.bot_verify AS verify, count(*) AS hits,
                        coalesce(sum(r.bytes), 0) AS bytes, count(*) FILTER (WHERE r.status >= 400) AS errors,
                        {SEEN}
                 FROM {BOT_ROWS} WHERE {filter} GROUP BY 1, 2, 3"
            ),
            order_by: "hits DESC, ip",
            totals: None,
        }],
    )
}

pub fn ai_crawlers(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    load_registry(&scope, store)?;
    let filter = format!("{} AND b.kind LIKE 'ai-%'", scope.filter);
    let mut by_bot = vec![
        col("operator", Text),
        col("bot", Text),
        col("kind", Text),
        col("hits", Integer),
        col("bytes", Integer),
        col("urls", Integer),
    ];
    by_bot.extend(verdict_columns());
    scope.notes.push(
        "Kinds: ai-training crawlers collect pages for model training, ai-search crawlers build an \
         assistant's search index, and ai-user fetches happen when a person asks an assistant about a page."
            .to_owned(),
    );
    scope.notes.push(VERIFY_NOTE.to_owned());
    scope.finish(
        "ai-crawlers",
        vec![
            TableSpec {
                name: "by_bot",
                columns: by_bot,
                sql: format!(
                    "SELECT b.operator, b.bot, b.kind, count(*) AS hits, coalesce(sum(r.bytes), 0) AS bytes,
                            count(DISTINCT r.path) AS urls, {VERDICTS}
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1, 2, 3"
                ),
                order_by: "hits DESC, bot",
                totals: None,
            },
            TableSpec {
                name: "by_kind",
                columns: vec![col("kind", Text), col("hits", Integer), col("bytes", Integer), col("urls", Integer)],
                sql: format!(
                    "SELECT b.kind, count(*) AS hits, coalesce(sum(r.bytes), 0) AS bytes,
                            count(DISTINCT r.path) AS urls
                     FROM {BOT_ROWS} WHERE {filter} GROUP BY 1"
                ),
                order_by: "hits DESC, kind",
                totals: None,
            },
            TableSpec {
                name: "top_urls",
                columns: vec![col("bot", Text), col("path", Text), col("hits", Integer)],
                sql: format!(
                    "SELECT b.bot, r.path, count(*) AS hits FROM {BOT_ROWS}
                     WHERE {filter} AND r.path IS NOT NULL GROUP BY 1, 2"
                ),
                order_by: "hits DESC, bot, path",
                totals: None,
            },
        ],
    )
}

pub fn undeclared(store: &Store, params: &ReportParams) -> Result<ReportResult> {
    let mut scope = Scope::new(store, params)?;
    scope.raw_only();
    let filter = format!(
        "{} AND u.category IN ('tool', 'library', 'empty', 'unknown')",
        scope.filter
    );
    let rows = "requests r JOIN user_agents u USING (ua_id)";
    scope.notes.push(
        "Tools, HTTP libraries, empty user agents and unrecognised ones. Browser user agents coming \
         from data-centre networks will be added here once network (ASN) data is available."
            .to_owned(),
    );
    scope.finish(
        "undeclared",
        vec![
            TableSpec {
                name: "by_agent",
                columns: vec![
                    col("category", Text),
                    col("family", Text),
                    col("hits", Integer),
                    col("ips", Integer),
                    col("errors", Integer),
                ],
                sql: format!(
                    "SELECT u.category, u.family, count(*) AS hits, count(DISTINCT r.ip) AS ips,
                            count(*) FILTER (WHERE r.status >= 400) AS errors
                     FROM {rows} WHERE {filter} GROUP BY 1, 2"
                ),
                order_by: "hits DESC, category, family",
                totals: None,
            },
            TableSpec {
                name: "by_ip",
                columns: vec![
                    col("ip", Text),
                    col("category", Text),
                    col("family", Text),
                    col("hits", Integer),
                    col("urls", Integer),
                    col("errors", Integer),
                    col("first_seen", Datetime),
                    col("last_seen", Datetime),
                ],
                sql: format!(
                    "SELECT r.ip, u.category, u.family, count(*) AS hits, count(DISTINCT r.path) AS urls,
                            count(*) FILTER (WHERE r.status >= 400) AS errors, {SEEN}
                     FROM {rows} WHERE {filter} GROUP BY 1, 2, 3"
                ),
                order_by: "hits DESC, ip",
                totals: None,
            },
        ],
    )
}
