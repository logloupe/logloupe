//! Reports. Every report is a function `fn(&Store, &ReportParams) -> Result<ReportResult>`
//! and returns the common envelope (spec 7.2), which the CLI and the app both
//! serialise unchanged.

use chrono::{NaiveDate, SecondsFormat, Utc};
use duckdb::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};

use crate::error::{Error, Result};
use crate::store::Store;

mod abuse;
pub mod blocklist;
mod bots;
mod traffic;

/// Version of the JSON envelope. Bump on breaking changes.
pub const ENVELOPE_VERSION: u32 = 1;

pub const DEFAULT_LIMIT: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    Hour,
    Day,
}

/// Optional filters shared by every report.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Filters {
    /// 2, 3, 4 or 5.
    pub status_class: Option<u8>,
    /// One exact response code, for example 504.
    pub status: Option<u16>,
    pub path_prefix: Option<String>,
    /// An address, or a /24 (IPv4) or /48 (IPv6) network.
    pub ip: Option<String>,
    pub source_type: Option<String>,
    pub country: Option<String>,
    pub asn: Option<u32>,
    /// A bot registry key or a user-agent family name.
    pub bot: Option<String>,
    /// `verified`, `fake` or `unverifiable`.
    pub verified: Option<String>,
    /// Referrer text to look for. `(none)` matches requests with no
    /// referrer; a leading `=` asks for an exact match.
    pub referrer: Option<String>,
    /// User-agent text to look for, with the same `(none)` and `=` forms.
    pub user_agent: Option<String>,
}

impl Filters {
    pub fn is_empty(&self) -> bool {
        *self == Filters::default()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ReportParams {
    /// First day, in the site's time zone. `None` means from the earliest data.
    pub from: Option<NaiveDate>,
    /// Last day, inclusive. `None` means to the latest data.
    pub to: Option<NaiveDate>,
    pub filters: Filters,
    pub granularity: Option<Granularity>,
    /// Return only this table.
    pub table: Option<String>,
    pub limit: u64,
    pub offset: u64,
}

impl Default for ReportParams {
    fn default() -> Self {
        ReportParams {
            from: None,
            to: None,
            filters: Filters::default(),
            granularity: None,
            table: None,
            limit: DEFAULT_LIMIT,
            offset: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnType {
    String,
    Integer,
    Float,
    Boolean,
    Date,
    Datetime,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Column {
    pub key: &'static str,
    #[serde(rename = "type")]
    pub ty: ColumnType,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Table {
    pub name: &'static str,
    pub columns: Vec<Column>,
    /// Cells are scalars only: string, number, boolean or null.
    pub rows: Vec<Vec<Json>>,
    pub totals: Map<String, Json>,
    /// Row count before paging.
    pub total_rows: u64,
    pub offset: u64,
    pub limit: u64,
}

/// Which days of the requested range were answered from raw rows, and how
/// many only exist as rollups (spec 3.5).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Coverage {
    pub raw_from: Option<NaiveDate>,
    pub raw_to: Option<NaiveDate>,
    pub rollup_days: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReportResult {
    pub report: &'static str,
    pub site: String,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub timezone: String,
    pub generated_at: String,
    pub schema_version: u32,
    pub coverage: Coverage,
    pub tables: Vec<Table>,
    pub notes: Vec<String>,
}

pub type ReportFn = fn(&Store, &ReportParams) -> Result<ReportResult>;

/// A report's entry in the registry.
pub struct ReportInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub tables: &'static [&'static str],
    pub run: ReportFn,
}

pub const REPORTS: &[ReportInfo] = &[
    ReportInfo {
        name: "overview",
        description: "Requests, pages, bytes, unique IPs, human/bot split and status classes per day or hour.",
        tables: &["series", "totals"],
        run: traffic::overview,
    },
    ReportInfo {
        name: "sources",
        description: "Pages by source type, referrer host and full referrer URL.",
        tables: &["by_source_type", "by_referrer_host", "by_referrer_url"],
        run: traffic::sources,
    },
    ReportInfo {
        name: "campaigns",
        description: "Pages by utm_source / utm_medium / utm_campaign, and by click ID.",
        tables: &["campaigns", "click_ids"],
        run: traffic::campaigns,
    },
    ReportInfo {
        name: "landing-pages",
        description: "First page of each approximate session (same IP and user agent, gaps under 30 minutes).",
        tables: &["landing_pages", "by_source_type"],
        run: traffic::landing_pages,
    },
    ReportInfo {
        name: "geography",
        description: "Pages and requests by country and by network (ASN).",
        tables: &["by_country", "by_asn"],
        run: traffic::geography,
    },
    ReportInfo {
        name: "ai-referrals",
        description: "Visits referred by AI assistants, by assistant and landing page.",
        tables: &["by_assistant", "by_landing_page"],
        run: traffic::ai_referrals,
    },
    ReportInfo {
        name: "bots",
        description: "Registered bots: hits, bytes, distinct URLs, errors and verification, by bot, kind and day.",
        tables: &["by_bot", "by_kind", "series"],
        run: bots::bots,
    },
    ReportInfo {
        name: "fake-bots",
        description: "Requests claiming a registered bot from outside its published IP ranges.",
        tables: &["by_bot", "by_ip", "paths"],
        run: bots::fake_bots,
    },
    ReportInfo {
        name: "bot-ips",
        description: "Activity per IP address for registered bots (use --bot to pick one).",
        tables: &["by_ip"],
        run: bots::bot_ips,
    },
    ReportInfo {
        name: "ai-crawlers",
        description: "AI training, AI search and user-triggered AI fetches, per operator, with top URLs.",
        tables: &["by_bot", "by_kind", "top_urls"],
        run: bots::ai_crawlers,
    },
    ReportInfo {
        name: "undeclared",
        description: "Tools, HTTP libraries, and empty or unrecognised user agents.",
        tables: &["by_agent", "by_ip"],
        run: bots::undeclared,
    },
    ReportInfo {
        name: "status-codes",
        description: "Response codes over time, per code, and the URLs and addresses behind the errors.",
        tables: &["by_status", "series", "error_paths", "error_ips"],
        run: abuse::status_codes,
    },
    ReportInfo {
        name: "top-ips",
        description: "The busiest addresses, networks, user agents and paths, with peak requests per minute.",
        tables: &[
            "by_ip",
            "by_network",
            "by_asn",
            "by_country",
            "by_user_agent",
            "by_path",
        ],
        run: abuse::top_ips,
    },
    ReportInfo {
        name: "referrers",
        description: "Referrers exactly as sent, internal ones included, with who sent them.",
        tables: &["by_referrer", "by_host"],
        run: abuse::referrers,
    },
    ReportInfo {
        name: "queries",
        description: "Query strings by shape, by parameter and by path, to spot loops and scraping.",
        tables: &["by_shape", "by_param", "by_path"],
        run: abuse::queries,
    },
    ReportInfo {
        name: "requests",
        description: "The individual requests behind the current filters, newest first.",
        tables: &["requests"],
        run: abuse::requests,
    },
];

pub fn run(store: &Store, name: &str, params: &ReportParams) -> Result<ReportResult> {
    let info = REPORTS
        .iter()
        .find(|r| r.name == name)
        .ok_or_else(|| Error::UnknownReport(name.to_owned()))?;
    if let Some(table) = &params.table
        && !info.tables.contains(&table.as_str())
    {
        return Err(Error::invalid(format!(
            "report '{name}' has no table '{table}'. Tables: {}",
            info.tables.join(", ")
        )));
    }
    (info.run)(store, params)
}

/// Quotes a string as a SQL literal.
pub(crate) fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// One table of a report: a query plus how to page, order and total it.
pub(crate) struct TableSpec {
    pub name: &'static str,
    pub columns: Vec<Column>,
    /// Produces one output column per entry in `columns`, named by its key.
    pub sql: String,
    pub order_by: &'static str,
    /// A query returning a single row of totals, with its columns.
    pub totals: Option<(Vec<Column>, String)>,
}

pub(crate) fn col(key: &'static str, ty: ColumnType) -> Column {
    Column { key, ty }
}

fn cast(column: &Column) -> String {
    let ty = match column.ty {
        ColumnType::Integer => "BIGINT",
        ColumnType::Float => "DOUBLE",
        ColumnType::Boolean => "BOOLEAN",
        ColumnType::String | ColumnType::Date | ColumnType::Datetime => "VARCHAR",
    };
    format!("CAST(q.\"{}\" AS {ty})", column.key)
}

fn read_cell(row: &duckdb::Row<'_>, index: usize, ty: ColumnType) -> duckdb::Result<Json> {
    Ok(match ty {
        ColumnType::Integer => row
            .get::<_, Option<i64>>(index)?
            .map_or(Json::Null, Json::from),
        ColumnType::Float => row
            .get::<_, Option<f64>>(index)?
            .map_or(Json::Null, Json::from),
        ColumnType::Boolean => row
            .get::<_, Option<bool>>(index)?
            .map_or(Json::Null, Json::from),
        ColumnType::String | ColumnType::Date | ColumnType::Datetime => row
            .get::<_, Option<String>>(index)?
            .map_or(Json::Null, Json::from),
    })
}

fn run_table(conn: &Connection, spec: TableSpec, limit: u64, offset: u64) -> Result<Table> {
    let casts: Vec<String> = spec.columns.iter().map(cast).collect();
    let sql = format!(
        "SELECT {}, count(*) OVER () FROM ({}) q ORDER BY {} LIMIT {limit} OFFSET {offset}",
        casts.join(", "),
        spec.sql,
        spec.order_by,
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut total_rows = 0u64;
    let mut rows = Vec::new();
    let mut result = stmt.query([])?;
    while let Some(row) = result.next()? {
        let mut cells = Vec::with_capacity(spec.columns.len());
        for (i, column) in spec.columns.iter().enumerate() {
            cells.push(read_cell(row, i, column.ty)?);
        }
        total_rows = row.get::<_, i64>(spec.columns.len())? as u64;
        rows.push(cells);
    }
    // Paged past the end: the window count never came back.
    if rows.is_empty() && offset > 0 {
        total_rows = conn.query_row(&format!("SELECT count(*) FROM ({}) q", spec.sql), [], |r| {
            r.get::<_, i64>(0)
        })? as u64;
    }

    let mut totals = Map::new();
    if let Some((columns, sql)) = &spec.totals {
        let casts: Vec<String> = columns.iter().map(cast).collect();
        let sql = format!("SELECT {} FROM ({sql}) q", casts.join(", "));
        conn.query_row(&sql, [], |row| {
            for (i, column) in columns.iter().enumerate() {
                totals.insert(column.key.to_owned(), read_cell(row, i, column.ty)?);
            }
            Ok(())
        })?;
    }

    Ok(Table {
        name: spec.name,
        columns: spec.columns,
        rows,
        totals,
        total_rows,
        offset,
        limit,
    })
}

/// Shared set-up for a report: the resolved date range, the WHERE clause for
/// raw rows, and coverage.
pub(crate) struct Scope<'a> {
    store: &'a Store,
    params: &'a ReportParams,
    pub from: NaiveDate,
    pub to: NaiveDate,
    timezone: String,
    /// Condition on `requests r` for the date range and every filter.
    pub filter: String,
    /// Days before this have only rollups.
    pub pruned_before: Option<NaiveDate>,
    pub coverage: Coverage,
    pub notes: Vec<String>,
}

fn parse_date(text: Option<String>) -> Option<NaiveDate> {
    text.and_then(|t| t.get(..10)?.parse().ok())
}

impl<'a> Scope<'a> {
    pub fn new(store: &'a Store, params: &'a ReportParams) -> Result<Self> {
        let conn = &store.conn;
        let settings = store.settings()?;
        let pruned_before = store
            .meta("pruned_before")?
            .and_then(|d| d.parse::<NaiveDate>().ok());

        // Default to everything stored, raw or rolled up.
        let (data_from, data_to) = conn.query_row(
            "SELECT CAST(min(d) AS VARCHAR), CAST(max(d) AS VARCHAR) FROM (
               SELECT CAST(min(ts_local) AS DATE) AS d FROM requests
               UNION ALL SELECT CAST(max(ts_local) AS DATE) FROM requests
               UNION ALL SELECT min(day) FROM daily_totals
               UNION ALL SELECT max(day) FROM daily_totals)",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )?;
        let today = Utc::now().with_timezone(&settings.timezone).date_naive();
        let from = params.from.or(parse_date(data_from)).unwrap_or(today);
        let to = params.to.or(parse_date(data_to)).unwrap_or(today);
        if from > to {
            return Err(Error::invalid(format!(
                "--from ({from}) is after --to ({to})"
            )));
        }

        let next = to.succ_opt().unwrap_or(to);
        let mut conditions = vec![format!(
            "r.ts_local >= TIMESTAMP '{from} 00:00:00' AND r.ts_local < TIMESTAMP '{next} 00:00:00'"
        )];
        let range = conditions[0].clone();
        filter_conditions(&params.filters, &mut conditions)?;

        let (raw_from, raw_to) = conn.query_row(
            &format!(
                "SELECT CAST(CAST(min(r.ts_local) AS DATE) AS VARCHAR),
                        CAST(CAST(max(r.ts_local) AS DATE) AS VARCHAR)
                 FROM requests r WHERE {range}"
            ),
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )?;
        let rollup_days = match pruned_before {
            Some(cutoff) => conn.query_row(
                &format!(
                    "SELECT count(*) FROM daily_totals
                     WHERE day >= DATE '{from}' AND day <= DATE '{to}' AND day < DATE '{cutoff}'"
                ),
                [],
                |row| row.get::<_, i64>(0),
            )? as u64,
            None => 0,
        };

        Ok(Scope {
            store,
            params,
            from,
            to,
            timezone: settings.timezone.name().to_owned(),
            filter: conditions.join(" AND "),
            pruned_before,
            coverage: Coverage {
                raw_from: parse_date(raw_from),
                raw_to: parse_date(raw_to),
                rollup_days,
            },
            notes: if settings.edge_cache {
                vec![
                    "This site is behind a caching CDN, so the log holds only the requests that reached \
                     the origin server. Counts are not total traffic, and lean towards bots and pages \
                     that cannot be cached."
                        .to_owned(),
                ]
            } else {
                Vec::new()
            },
        })
    }

    pub fn conn(&self) -> &Connection {
        &self.store.conn
    }

    /// For reports that can only read raw rows: say so when part of the range
    /// has been pruned, rather than return a silently partial answer.
    pub fn raw_only(&mut self) {
        if self.coverage.rollup_days == 0 {
            return;
        }
        let covered = match (self.coverage.raw_from, self.coverage.raw_to) {
            (Some(a), Some(b)) => format!("It covers {a} to {b} only."),
            _ => "No raw rows remain in the range, so it is empty.".to_owned(),
        };
        self.notes.push(format!(
            "{} day(s) in the range have been pruned to daily rollups, which this report cannot use. {covered}",
            self.coverage.rollup_days
        ));
    }

    pub fn finish(self, report: &'static str, specs: Vec<TableSpec>) -> Result<ReportResult> {
        let mut tables = Vec::with_capacity(specs.len());
        for spec in specs {
            if self.params.table.as_deref().is_none_or(|t| t == spec.name) {
                tables.push(run_table(
                    self.conn(),
                    spec,
                    self.params.limit,
                    self.params.offset,
                )?);
            }
        }
        Ok(ReportResult {
            report,
            site: self.store.slug().to_owned(),
            from: self.from,
            to: self.to,
            timezone: self.timezone,
            generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            schema_version: ENVELOPE_VERSION,
            coverage: self.coverage,
            tables,
            notes: self.notes,
        })
    }
}

fn filter_conditions(f: &Filters, out: &mut Vec<String>) -> Result<()> {
    if let Some(class) = f.status_class {
        if !(1..=5).contains(&class) {
            return Err(Error::invalid(
                "status class must be 1 to 5 (for example 4 for 4xx)",
            ));
        }
        let low = u32::from(class) * 100;
        out.push(format!("r.status BETWEEN {low} AND {}", low + 99));
    }
    if let Some(status) = f.status {
        out.push(format!("r.status = {status}"));
    }
    if let Some(prefix) = &f.path_prefix {
        out.push(format!("starts_with(r.path, {})", quote(prefix)));
    }
    if let Some(ip) = &f.ip {
        out.push(ip_condition(ip)?);
    }
    if let Some(source) = &f.source_type {
        out.push(format!(
            "r.source_type = {}",
            quote(&source.to_ascii_lowercase())
        ));
    }
    if let Some(country) = &f.country {
        out.push(format!(
            "r.country = {}",
            quote(&country.to_ascii_uppercase())
        ));
    }
    if let Some(asn) = f.asn {
        out.push(format!("r.asn = {asn}"));
    }
    if let Some(bot) = &f.bot {
        let bot = quote(&bot.to_ascii_lowercase());
        out.push(format!(
            "r.ua_id IN (SELECT ua_id FROM user_agents WHERE lower(bot_key) = {bot} OR lower(family) = {bot})"
        ));
    }
    if let Some(verified) = &f.verified {
        out.push(format!(
            "r.bot_verify = {}",
            quote(&verified.to_ascii_lowercase())
        ));
    }
    if let Some(referrer) = &f.referrer {
        out.push(text_condition("r.referer", referrer));
    }
    if let Some(user_agent) = &f.user_agent {
        let inner = match user_agent.as_str() {
            "(none)" => "ua = ''".to_owned(),
            _ => text_condition("ua", user_agent),
        };
        out.push(format!(
            "r.ua_id IN (SELECT ua_id FROM user_agents WHERE {inner})"
        ));
    }
    Ok(())
}

/// A text filter: `(none)` matches a missing value, a leading `=` asks for an
/// exact match, and anything else matches when contained (ignoring case).
fn text_condition(column: &str, value: &str) -> String {
    if value == "(none)" {
        format!("{column} IS NULL")
    } else if let Some(exact) = value.strip_prefix('=') {
        format!("{column} = {}", quote(exact))
    } else {
        format!(
            "contains(lower({column}), {})",
            quote(&value.to_lowercase())
        )
    }
}

/// An exact address, or a /24 or /48 network matched through `ip_net`.
fn ip_condition(text: &str) -> Result<String> {
    use std::net::IpAddr;
    let bad = || {
        Error::invalid(format!(
            "invalid IP filter '{text}': give an address, an IPv4 /24 or an IPv6 /48"
        ))
    };
    let Some((addr, prefix)) = text.split_once('/') else {
        let ip: IpAddr = text.parse().map_err(|_| bad())?;
        return Ok(format!("r.ip = {}", quote(&ip.to_canonical().to_string())));
    };
    let ip: IpAddr = addr.parse().map_err(|_| bad())?;
    let net = match (ip.to_canonical(), prefix) {
        (IpAddr::V4(v4), "24") => {
            let [a, b, c, _] = v4.octets();
            format!("{a}.{b}.{c}.0/24")
        }
        (IpAddr::V6(v6), "48") => {
            let s = v6.segments();
            format!(
                "{}/48",
                std::net::Ipv6Addr::new(s[0], s[1], s[2], 0, 0, 0, 0, 0)
            )
        }
        _ => return Err(bad()),
    };
    Ok(format!("r.ip_net = {}", quote(&net)))
}

/// Log values (paths, referrers, user agents) are written by whoever sent
/// the request. A spreadsheet treats a cell starting with `=`, `+`, `-` or
/// `@` as a formula, so those get a leading apostrophe to keep them as text.
fn csv_field(text: &str) -> String {
    let guarded;
    let text = if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        guarded = format!("'{text}");
        guarded.as_str()
    } else {
        text
    };
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

/// Renders one table as CSV (RFC 4180 quoting), header row first.
pub fn to_csv(table: &Table) -> String {
    let mut out = String::new();
    let header: Vec<String> = table.columns.iter().map(|c| csv_field(c.key)).collect();
    out.push_str(&header.join(","));
    out.push_str("\r\n");
    for row in &table.rows {
        let cells: Vec<String> = row
            .iter()
            .map(|value| match value {
                Json::Null => String::new(),
                Json::String(s) => csv_field(s),
                other => other.to_string(),
            })
            .collect();
        out.push_str(&cells.join(","));
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
