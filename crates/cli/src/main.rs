//! `logloupe`: the command-line front end. Argument parsing and output only; all
//! logic lives in `logloupe-core`.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use chrono::{DateTime, NaiveDate};
use clap::{Args, Parser, Subcommand, ValueEnum};
use logloupe_core::bots::{Registry, ranges as bot_ranges};
use logloupe_core::ingest::{self, FileReport, FileStatus, IngestOptions};
use logloupe_core::remote::{self, FetchOptions, FileState, Phase, Remote, Ssh};
use logloupe_core::report::{self, Filters, Granularity, ReportParams, ReportResult, Table};
use logloupe_core::store::{DataDir, Mode, OpenOptions, SettingsPatch, SiteSettings, Store};
use logloupe_core::{Error, Result, maintain};
use serde_json::{Value as Json, json};

mod output;

use output::{bytes, number, print_table};

/// Exit code when another process holds the site's database (EX_TEMPFAIL).
const EXIT_LOCKED: u8 = 75;

#[derive(Parser)]
#[command(name = "logloupe", version, about = "Analyse web server access logs.")]
struct Cli {
    #[command(flatten)]
    global: Global,
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct Global {
    /// Data directory (default: LOGLOUPE_DATA_DIR, then the per-user data directory)
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<PathBuf>,
    /// DuckDB memory limit, for example 512MB or 2GB (default 512MB)
    #[arg(long, global = true, value_name = "SIZE", value_parser = parse_memory)]
    memory_limit: Option<u64>,
    /// Worker threads (default: cores - 1)
    #[arg(long, global = true, value_name = "N")]
    threads: Option<usize>,
    /// Keep retrying for this long when another process has the site open
    #[arg(long, global = true, value_name = "SECONDS", default_value_t = 0)]
    wait_lock: u64,
    /// No progress or notes on stderr
    #[arg(long, global = true)]
    quiet: bool,
    /// Write one JSON document to stdout instead of text
    #[arg(long, global = true)]
    json: bool,
    /// Never fetch anything: use only the cached bot IP ranges
    #[arg(long, global = true)]
    offline: bool,
    /// Check bots by reverse DNS as well (looks up addresses with your DNS resolver)
    #[arg(long, global = true)]
    dns: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Add, list, change and remove sites
    #[command(subcommand)]
    Site(SiteCommand),
    /// Ingest log files (plain or gzip) into a site
    Ingest(IngestArgs),
    /// Fetch new and changed logs from the site's server over SSH and ingest them
    Fetch(FetchArgs),
    /// The server a site's logs are fetched from
    #[command(subcommand)]
    Remote(RemoteCommand),
    /// Run a report
    Report(ReportArgs),
    /// List available reports
    Reports,
    /// Storage: sizes, ingested files, pruning, compaction
    #[command(subcommand)]
    Db(DbCommand),
    /// Published bot IP ranges: fetch them or show what is cached
    #[command(subcommand)]
    Ranges(RangesCommand),
    /// Re-run user-agent classification, bot verification and geo lookup over stored rows
    Reenrich { slug: String },
    /// Country and network (ASN) databases: download DB-IP Lite or show what is installed
    #[command(subcommand)]
    Geo(GeoCommand),
    /// Print the busiest addresses or networks as a block list to paste into a server config
    Blocklist(BlocklistArgs),
}

#[derive(Args)]
struct BlocklistArgs {
    slug: String,
    /// List single addresses (ip) or their /24 and /48 networks (network)
    #[arg(long, default_value = "ip")]
    by: String,
    /// plain, nginx or apache
    #[arg(long, default_value = "plain")]
    style: String,
    /// Only addresses or networks with at least this many requests
    #[arg(long = "min-hits", default_value_t = 100)]
    min_hits: u64,
    /// At most this many entries, busiest first
    #[arg(long, default_value_t = 1000)]
    limit: u64,
    #[command(flatten)]
    filter: FilterArgs,
}

#[derive(Subcommand)]
enum GeoCommand {
    /// Download this month's free DB-IP Lite country and ASN databases
    Update,
    /// Show which databases are installed
    Status,
}

#[derive(Subcommand)]
enum RangesCommand {
    /// Fetch every operator's published lists
    Update,
    /// Show the cached lists and when they were fetched
    Status,
}

#[derive(Args)]
struct SiteOptions {
    /// One of the site's own hostnames; repeat for each (www and bare domain)
    #[arg(long = "host", value_name = "HOST")]
    hosts: Vec<String>,
    /// IANA time zone used for days and hours, for example Europe/London
    #[arg(long)]
    timezone: Option<String>,
    /// forever, none, or days:N
    #[arg(long)]
    retention: Option<String>,
    /// What to do with lines inside a time range already ingested: error, skip or allow
    #[arg(long)]
    overlap: Option<String>,
    /// Which address is the client: auto, peer or forwarded
    #[arg(long = "client-ip")]
    client_ip: Option<String>,
    /// The site is behind a caching CDN, so the log holds origin traffic only
    #[arg(long = "edge-cache", value_name = "true|false")]
    edge_cache: Option<bool>,
}

#[derive(Subcommand)]
enum SiteCommand {
    /// Create a site
    Add {
        slug: String,
        #[command(flatten)]
        options: SiteOptions,
    },
    /// List sites
    List,
    /// Show one site's settings and contents
    Show { slug: String },
    /// Change a site's settings
    Set {
        slug: String,
        #[command(flatten)]
        options: SiteOptions,
    },
    /// Delete a site and all its data
    Remove { slug: String },
}

#[derive(Args)]
struct IngestArgs {
    slug: String,
    /// Files or glob patterns
    #[arg(required = true)]
    files: Vec<String>,
    /// Log format: auto or a format name
    #[arg(long, default_value = "auto")]
    format: String,
    /// Re-ingest files that were already ingested, replacing their rows
    #[arg(long)]
    force: bool,
    /// Override the site's overlap policy: error, skip or allow
    #[arg(long)]
    overlap: Option<String>,
    /// Skip reverse DNS lookups
    #[arg(long = "no-dns")]
    no_dns: bool,
}

#[derive(Args)]
struct FetchArgs {
    slug: String,
    /// Leave out files not modified in the last N days
    #[arg(long, value_name = "N")]
    days: Option<u32>,
    /// Fetch files again even when they have not changed, replacing their rows
    #[arg(long)]
    force: bool,
    /// Log format: auto or a format name
    #[arg(long, default_value = "auto")]
    format: String,
    /// Override the site's overlap policy: error, skip or allow
    #[arg(long)]
    overlap: Option<String>,
    /// Skip reverse DNS lookups
    #[arg(long = "no-dns")]
    no_dns: bool,
}

#[derive(Subcommand)]
enum RemoteCommand {
    /// Set the server and the log paths on it
    Set {
        slug: String,
        /// An SSH host alias from your SSH config, or user@host.
        /// `ssh <host>` must already work without asking for anything
        #[arg(long)]
        host: String,
        /// Port, when it is not 22 and not in your SSH config
        #[arg(long)]
        port: Option<u16>,
        /// A path on the server, with wildcards; repeat for each.
        /// Quote it so your own shell does not expand it
        #[arg(long = "path", value_name = "PATTERN", required = true)]
        paths: Vec<String>,
    },
    /// Show the server set for a site
    Show { slug: String },
    /// Forget the server; logs already fetched stay
    Remove { slug: String },
    /// List the files on the server and what a fetch would do with each
    List {
        slug: String,
        /// Mark files not modified in the last N days as old
        #[arg(long, value_name = "N")]
        days: Option<u32>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ReportFormat {
    Json,
    Table,
    Csv,
}

#[derive(Clone, Copy, ValueEnum)]
enum GranularityArg {
    Hour,
    Day,
}

#[derive(Args)]
struct FilterArgs {
    /// First day, in the site's time zone (default: earliest data)
    #[arg(long, value_name = "DATE")]
    from: Option<NaiveDate>,
    /// Last day, inclusive (default: latest data)
    #[arg(long, value_name = "DATE")]
    to: Option<NaiveDate>,
    /// Status class: 2, 3, 4 or 5
    #[arg(long, value_name = "CLASS")]
    status: Option<u8>,
    /// One exact response code, for example 504
    #[arg(long = "status-code", value_name = "CODE")]
    status_code: Option<u16>,
    #[arg(long = "path-prefix", value_name = "PREFIX")]
    path_prefix: Option<String>,
    /// An address, an IPv4 /24 or an IPv6 /48
    #[arg(long, value_name = "IP")]
    ip: Option<String>,
    /// direct, internal, search, ai, social, email, campaign or other
    #[arg(long = "source", value_name = "TYPE")]
    source_type: Option<String>,
    #[arg(long, value_name = "ISO")]
    country: Option<String>,
    #[arg(long)]
    asn: Option<u32>,
    /// A bot key or user-agent family, for example googlebot
    #[arg(long)]
    bot: Option<String>,
    /// verified, fake or unverifiable
    #[arg(long)]
    verified: Option<String>,
    /// Referrer contains this text; "(none)" for no referrer; start with = for an exact match
    #[arg(long, value_name = "TEXT")]
    referrer: Option<String>,
    /// User agent contains this text; "(none)" for none; start with = for an exact match
    #[arg(long = "user-agent", value_name = "TEXT")]
    user_agent: Option<String>,
}

impl FilterArgs {
    /// The date range and filters as report parameters.
    fn params(self) -> ReportParams {
        ReportParams {
            from: self.from,
            to: self.to,
            filters: Filters {
                status_class: self.status,
                status: self.status_code,
                path_prefix: self.path_prefix,
                ip: self.ip,
                source_type: self.source_type,
                country: self.country,
                asn: self.asn,
                bot: self.bot,
                verified: self.verified,
                referrer: self.referrer,
                user_agent: self.user_agent,
            },
            ..ReportParams::default()
        }
    }
}

#[derive(Args)]
struct ReportArgs {
    slug: String,
    /// Report name (see `logloupe reports`)
    name: String,
    #[command(flatten)]
    filter: FilterArgs,
    /// Bucket size for time series
    #[arg(long, value_enum)]
    granularity: Option<GranularityArg>,
    /// Bursts report: requests a minute from one address that count as a burst
    #[arg(long, value_name = "RATE")]
    threshold: Option<u64>,
    /// Return only this table
    #[arg(long, value_name = "NAME")]
    table: Option<String>,
    /// Rows per table
    #[arg(long, default_value_t = report::DEFAULT_LIMIT)]
    limit: u64,
    #[arg(long, default_value_t = 0)]
    offset: u64,
    #[arg(long, value_enum, default_value = "table")]
    format: ReportFormat,
}

#[derive(Subcommand)]
enum DbCommand {
    /// Size on disk and contents, for one site or all
    Stats { slug: Option<String> },
    /// List the files ingested into a site
    Files { slug: String },
    /// Delete raw rows before a date; rollups are kept unless --rollups
    Prune {
        slug: String,
        #[arg(long, value_name = "DATE")]
        before: NaiveDate,
        /// Delete those days' rollups as well
        #[arg(long)]
        rollups: bool,
    },
    /// Delete one ingested file's rows
    DeleteFile { slug: String, file_id: i32 },
    /// Delete all rows and rollups but keep the site
    Clear { slug: String },
    /// Rewrite the database to reclaim disk space after deletes
    Compact { slug: String },
}

fn parse_memory(text: &str) -> std::result::Result<u64, String> {
    let lower = text.trim().to_ascii_lowercase();
    let (digits, scale) = if let Some(n) = lower.strip_suffix("gb") {
        (n, 1024)
    } else if let Some(n) = lower.strip_suffix("mb") {
        (n, 1)
    } else {
        (lower.as_str(), 1)
    };
    digits
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .map(|n| n * scale)
        .ok_or_else(|| {
            format!(
                "invalid size '{text}': use a number of megabytes, or a value like 512MB or 2GB"
            )
        })
}

struct App {
    dir: DataDir,
    opts: OpenOptions,
    quiet: bool,
    json: bool,
    offline: bool,
    dns: bool,
}

impl App {
    fn open(&self, slug: &str, mode: Mode) -> Result<Store> {
        Store::open(&self.dir, slug, mode, &self.opts)
    }

    fn note(&self, message: &str) {
        if !self.quiet {
            eprintln!("{message}");
        }
    }

    fn emit(&self, value: &Json) {
        println!(
            "{}",
            serde_json::to_string(value).expect("JSON values serialise")
        );
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.global.json
        || matches!(&cli.command, Command::Report(args) if args.format == ReportFormat::Json);
    match run(cli, json) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if json {
                println!(
                    "{}",
                    json!({
                        "schema_version": report::ENVELOPE_VERSION,
                        "error": { "code": error.code(), "message": error.to_string() },
                    })
                );
            }
            eprintln!("error: {error}");
            ExitCode::from(match error {
                Error::SiteLocked(_) => EXIT_LOCKED,
                _ => 1,
            })
        }
    }
}

fn run(cli: Cli, json: bool) -> Result<()> {
    let defaults = OpenOptions::default();
    let app = App {
        dir: DataDir::resolve(cli.global.data_dir)?,
        opts: OpenOptions {
            memory_limit_mb: cli.global.memory_limit.unwrap_or(defaults.memory_limit_mb),
            threads: cli.global.threads,
            wait_lock: Duration::from_secs(cli.global.wait_lock),
        },
        quiet: cli.global.quiet,
        json,
        offline: cli.global.offline,
        dns: cli.global.dns && !cli.global.offline,
    };
    match cli.command {
        Command::Site(command) => site(&app, command),
        Command::Ingest(args) => run_ingest(&app, args),
        Command::Fetch(args) => run_fetch(&app, args),
        Command::Remote(command) => remote_command(&app, command),
        Command::Report(args) => run_report(&app, args),
        Command::Reports => list_reports(&app),
        Command::Db(command) => db(&app, command),
        Command::Ranges(command) => ranges(&app, command),
        Command::Reenrich { slug } => reenrich(&app, &slug),
        Command::Geo(command) => geo(&app, command),
        Command::Blocklist(args) => blocklist(&app, args),
    }
}

/// The settings changes asked for on the command line.
fn patch(options: &SiteOptions) -> SettingsPatch {
    SettingsPatch {
        hosts: (!options.hosts.is_empty()).then(|| options.hosts.clone()),
        timezone: options.timezone.clone(),
        retention: options.retention.clone(),
        overlap: options.overlap.clone(),
        client_ip: options.client_ip.clone(),
        edge_cache: options.edge_cache,
    }
}

fn print_stats(stats: &maintain::SiteStats) {
    let or_dash = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".to_owned());
    let pairs = [
        ("site", stats.slug.clone()),
        ("hosts", stats.hosts.join(", ")),
        ("time zone", stats.timezone.clone()),
        ("retention", stats.retention.clone()),
        ("overlap", stats.overlap.clone()),
        ("client IP", stats.client_ip.clone()),
        (
            "behind CDN cache",
            if stats.edge_cache { "yes" } else { "no" }.to_owned(),
        ),
        ("size on disk", bytes(stats.size_bytes)),
        ("rows", number(stats.rows)),
        ("first request", or_dash(&stats.first_ts)),
        ("last request", or_dash(&stats.last_ts)),
        ("ingested files", number(stats.files)),
        ("user agents", number(stats.user_agents)),
        ("rollup-only days", number(stats.rollup_only_days)),
    ];
    for (label, value) in pairs {
        println!("{label:<17}{value}");
    }
}

/// Stats for every site. A site that another process has open is reported
/// with its size only.
fn all_stats(app: &App) -> Result<Vec<Json>> {
    Ok(maintain::all_stats(&app.dir, &app.opts)?
        .into_iter()
        .map(|entry| match entry.stats {
            Some(stats) => serde_json::to_value(stats).expect("stats serialise"),
            None => json!({ "slug": entry.slug, "size_bytes": entry.size_bytes, "locked": true }),
        })
        .collect())
}

fn print_stats_list(all: &[Json]) {
    let text = |v: &Json| match v {
        Json::String(s) => s.clone(),
        Json::Null => "-".to_owned(),
        other => other.to_string(),
    };
    let rows: Vec<Vec<String>> = all
        .iter()
        .map(|s| {
            if s["locked"] == true {
                return vec![
                    text(&s["slug"]),
                    "(in use)".into(),
                    "-".into(),
                    "-".into(),
                    bytes(s["size_bytes"].as_u64().unwrap_or(0)),
                    "-".into(),
                ];
            }
            vec![
                text(&s["slug"]),
                number(s["rows"].as_u64().unwrap_or(0)),
                text(&s["first_ts"]),
                text(&s["last_ts"]),
                bytes(s["size_bytes"].as_u64().unwrap_or(0)),
                text(&s["retention"]),
            ]
        })
        .collect();
    if rows.is_empty() {
        println!("No sites yet. Add one with: logloupe site add <slug> --host example.com");
        return;
    }
    print_table(
        &[
            "site",
            "rows",
            "first request",
            "last request",
            "size",
            "retention",
        ],
        &rows,
    );
    let total: u64 = all.iter().filter_map(|s| s["size_bytes"].as_u64()).sum();
    println!("\ntotal size on disk: {}", bytes(total));
}

fn site(app: &App, command: SiteCommand) -> Result<()> {
    match command {
        SiteCommand::Add { slug, options } => {
            if options.hosts.is_empty() {
                return Err(Error::Invalid(
                    "give at least one --host, for example --host example.com --host www.example.com".into(),
                ));
            }
            let mut settings = SiteSettings::default();
            patch(&options).apply(&mut settings)?;
            let store = Store::create(&app.dir, &slug, &settings, &app.opts)?;
            let stats = maintain::stats(&app.dir, &store)?;
            if app.json {
                app.emit(&serde_json::to_value(&stats).expect("stats serialise"));
            } else {
                println!("Created site '{slug}'.");
                print_stats(&stats);
            }
        }
        SiteCommand::List => {
            let all = all_stats(app)?;
            if app.json {
                app.emit(&json!({ "sites": all }));
            } else {
                print_stats_list(&all);
            }
        }
        SiteCommand::Show { slug } => {
            let store = app.open(&slug, Mode::ReadOnly)?;
            let stats = maintain::stats(&app.dir, &store)?;
            if app.json {
                app.emit(&serde_json::to_value(&stats).expect("stats serialise"));
            } else {
                print_stats(&stats);
            }
        }
        SiteCommand::Set { slug, options } => {
            let store = app.open(&slug, Mode::ReadWrite)?;
            let change = maintain::update_settings(&store, &patch(&options))?;
            if change.stale_rollup_days > 0 {
                app.note(&format!(
                    "note: {} day(s) exist only as rollups and keep their old day boundaries.",
                    change.stale_rollup_days
                ));
            }
            if change.hosts_changed {
                app.note(
                    "note: rows already ingested keep their old source types. Re-ingest with --force to apply the new hostnames.",
                );
            }
            let stats = maintain::stats(&app.dir, &store)?;
            if app.json {
                app.emit(&serde_json::to_value(&stats).expect("stats serialise"));
            } else {
                print_stats(&stats);
            }
        }
        SiteCommand::Remove { slug } => {
            let freed = app.dir.site_size(&slug);
            app.dir.remove_site(&slug)?;
            if app.json {
                app.emit(&json!({ "removed": slug, "bytes_freed": freed }));
            } else {
                println!("Removed site '{slug}' ({} freed).", bytes(freed));
            }
        }
    }
    Ok(())
}

/// Expands glob patterns. A pattern that matches nothing is an error, so a
/// typo does not pass silently.
fn expand_files(patterns: &[String]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for pattern in patterns {
        let literal = PathBuf::from(pattern);
        if literal.is_file() {
            files.push(literal);
            continue;
        }
        let matches = glob::glob(pattern)
            .map_err(|e| Error::Invalid(format!("invalid pattern '{pattern}': {e}")))?;
        let before = files.len();
        for entry in matches {
            let path = entry.map_err(|e| Error::Invalid(e.to_string()))?;
            if path.is_file() {
                files.push(path);
            }
        }
        if files.len() == before {
            return Err(Error::Invalid(format!("no files match '{pattern}'")));
        }
        files[before..].sort();
    }
    Ok(files)
}

fn iso(ts: Option<i64>) -> String {
    ts.and_then(|t| DateTime::from_timestamp(t, 0))
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "-".to_owned())
}

fn run_ingest(app: &App, args: IngestArgs) -> Result<()> {
    let files = expand_files(&args.files)?;
    let options = IngestOptions {
        format: (args.format != "auto").then(|| args.format.clone()),
        force: args.force,
        overlap: args.overlap.as_deref().map(str::parse).transpose()?,
        dns: app.dns && !args.no_dns,
    };

    refresh_ranges(app);
    let mut store = app.open(&args.slug, Mode::ReadWrite)?;
    let show_progress = !app.quiet && std::io::stderr().is_terminal();
    let mut progress = |p: ingest::Progress<'_>| {
        if show_progress {
            let percent = (p.bytes_done * 100)
                .checked_div(p.bytes_total)
                .unwrap_or(100);
            let name = p.path.file_name().unwrap_or_default().to_string_lossy();
            eprint!("\r{name}: {percent:>3}%  {} rows   ", number(p.rows));
            let _ = std::io::stderr().flush();
        }
    };
    let cancel = AtomicBool::new(false);
    let result = ingest::ingest(&mut store, &files, &options, &mut progress, &cancel);
    if show_progress {
        eprint!("\r{:60}\r", "");
    }
    let reports = result?;

    if app.json {
        app.emit(&json!({ "site": args.slug, "files": reports }));
        return Ok(());
    }
    for report in &reports {
        print_file_report(app, report);
    }
    let total: u64 = reports.iter().map(|r| r.rows_ok).sum();
    let seconds: f64 = reports.iter().map(|r| r.seconds).sum();
    if reports.len() > 1 {
        println!(
            "{} files, {} rows added in {seconds:.1}s",
            reports.len(),
            number(total)
        );
    }
    Ok(())
}

fn print_file_report(app: &App, report: &FileReport) {
    let name = report
        .path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    match report.status {
        FileStatus::Unchanged => println!("{name}: already ingested, nothing new"),
        FileStatus::Empty => println!("{name}: empty"),
        FileStatus::Ingested | FileStatus::Resumed => {
            let verb = if report.status == FileStatus::Resumed {
                "resumed, added"
            } else {
                "ingested"
            };
            let megabytes = report.size_bytes as f64 / 1e6;
            println!(
                "{name}: {verb} {} rows ({} bad, {} skipped) in {:.1}s [{}{}]  {} to {} UTC  {:.0} MB/s",
                number(report.rows_ok),
                number(report.rows_bad),
                number(report.rows_skipped),
                report.seconds,
                report.format.as_deref().unwrap_or("?"),
                if report.gzip { ", gzip" } else { "" },
                iso(report.first_ts),
                iso(report.last_ts),
                if report.seconds > 0.0 {
                    megabytes / report.seconds
                } else {
                    0.0
                },
            );
        }
    }
    if !report.bad_samples.is_empty() {
        app.note(&format!("  first bad lines in {name}:"));
        for bad in report.bad_samples.iter().take(5) {
            app.note(&format!("    {}: {}", bad.reason, bad.line));
        }
    }
}

/// A serialisable result with the site's name added, for `--json`.
fn with_site(slug: &str, value: &impl serde::Serialize) -> Json {
    let mut json = serde_json::to_value(value).expect("results serialise");
    if let Json::Object(map) = &mut json {
        map.insert("site".to_owned(), Json::String(slug.to_owned()));
    }
    json
}

fn need_network(app: &App, what: &str) -> Result<()> {
    if app.offline {
        return Err(Error::Invalid(format!(
            "{what} connects to the server; remove --offline"
        )));
    }
    Ok(())
}

fn print_remote(remote: &Remote) {
    println!("{:<8}{}", "server", remote.host);
    if let Some(port) = remote.port {
        println!("{:<8}{port}", "port");
    }
    for path in &remote.paths {
        println!("{:<8}{path}", "path");
    }
}

fn remote_command(app: &App, command: RemoteCommand) -> Result<()> {
    match command {
        RemoteCommand::Set {
            slug,
            host,
            port,
            paths,
        } => {
            let store = app.open(&slug, Mode::ReadWrite)?;
            let stored = remote::set(&store, &Remote { host, port, paths })?;
            if app.json {
                app.emit(&json!({ "site": slug, "remote": stored }));
            } else {
                print_remote(&stored);
                println!(
                    "\nCheck it with `logloupe remote list {slug}`, then run `logloupe fetch {slug}`."
                );
            }
        }
        RemoteCommand::Show { slug } => {
            let store = app.open(&slug, Mode::ReadOnly)?;
            let found = remote::get(&store)?;
            if app.json {
                app.emit(&json!({ "site": slug, "remote": found }));
            } else {
                match found {
                    Some(found) => print_remote(&found),
                    None => println!(
                        "No server set. Add one with: logloupe remote set {slug} --host <alias> --path '<pattern>'"
                    ),
                }
            }
        }
        RemoteCommand::Remove { slug } => {
            let store = app.open(&slug, Mode::ReadWrite)?;
            let removed = remote::remove(&store)?;
            if app.json {
                app.emit(&json!({ "site": slug, "removed": removed }));
            } else if removed {
                println!("Removed the server from '{slug}'. Logs already fetched are kept.");
            } else {
                println!("'{slug}' had no server set.");
            }
        }
        RemoteCommand::List { slug, days } => {
            need_network(app, "remote list")?;
            let store = app.open(&slug, Mode::ReadOnly)?;
            let listing = remote::list(&store, days, &Ssh::new())?;
            if app.json {
                app.emit(&with_site(&slug, &listing));
                return Ok(());
            }
            if listing.files.is_empty() {
                println!("No files on {} match.", listing.host);
            } else {
                let rows: Vec<Vec<String>> = listing
                    .files
                    .iter()
                    .map(|f| {
                        vec![
                            match f.state {
                                FileState::New => "new",
                                FileState::Changed => "changed",
                                FileState::Fetched => "fetched",
                                FileState::Old => "old",
                            }
                            .to_owned(),
                            bytes(f.size_bytes),
                            iso(Some(f.modified)),
                            f.path.clone(),
                        ]
                    })
                    .collect();
                print_table(&["state", "size", "modified (UTC)", "path"], &rows);
                let pending = listing
                    .files
                    .iter()
                    .filter(|f| matches!(f.state, FileState::New | FileState::Changed));
                println!(
                    "\n`logloupe fetch {slug}` would download {} file(s), {}.",
                    pending.clone().count(),
                    bytes(pending.map(|f| f.size_bytes).sum())
                );
            }
            for note in &listing.notes {
                app.note(&format!("note: {note}"));
            }
        }
    }
    Ok(())
}

fn run_fetch(app: &App, args: FetchArgs) -> Result<()> {
    need_network(app, "fetch")?;
    let options = FetchOptions {
        days: args.days,
        ingest: IngestOptions {
            format: (args.format != "auto").then(|| args.format.clone()),
            force: args.force,
            overlap: args.overlap.as_deref().map(str::parse).transpose()?,
            dns: app.dns && !args.no_dns,
        },
    };

    refresh_ranges(app);
    let mut store = app.open(&args.slug, Mode::ReadWrite)?;
    let show_progress = !app.quiet && std::io::stderr().is_terminal();
    let mut progress = |p: remote::Progress<'_>| {
        if show_progress {
            let percent = (p.bytes_done * 100)
                .checked_div(p.bytes_total)
                .unwrap_or(100);
            let name = p.path.rsplit('/').next().unwrap_or(p.path);
            let doing = match p.phase {
                Phase::Download => "downloading".to_owned(),
                Phase::Ingest => format!("ingesting, {} rows", number(p.rows)),
            };
            eprint!(
                "\r[{}/{}] {name}: {percent:>3}% {doing}      ",
                p.file, p.files
            );
            let _ = std::io::stderr().flush();
        }
    };
    let cancel = AtomicBool::new(false);
    let result = remote::fetch(&mut store, &options, &Ssh::new(), &mut progress, &cancel);
    if show_progress {
        eprint!("\r{:78}\r", "");
    }
    let report = result?;

    if app.json {
        app.emit(&with_site(&args.slug, &report));
        return Ok(());
    }
    for file in &report.files {
        print_file_report(app, file);
    }
    let total: u64 = report.files.iter().map(|r| r.rows_ok).sum();
    let mut summary = format!(
        "Fetched {} file(s) from {} ({}), {} rows added",
        report.files.len(),
        report.host,
        bytes(report.bytes_downloaded),
        number(total)
    );
    if report.already_fetched > 0 {
        summary.push_str(&format!("; {} unchanged", report.already_fetched));
    }
    if report.too_old > 0 {
        summary.push_str(&format!("; {} older than --days left out", report.too_old));
    }
    println!("{summary}.");
    for note in &report.notes {
        app.note(&format!("note: {note}"));
    }
    Ok(())
}

fn run_report(app: &App, args: ReportArgs) -> Result<()> {
    let params = ReportParams {
        granularity: args.granularity.map(|g| match g {
            GranularityArg::Hour => Granularity::Hour,
            GranularityArg::Day => Granularity::Day,
        }),
        threshold: args.threshold,
        table: args.table,
        limit: args.limit,
        offset: args.offset,
        ..args.filter.params()
    };
    let store = app.open(&args.slug, Mode::ReadOnly)?;
    let result = report::run(&store, &args.name, &params)?;
    match args.format {
        ReportFormat::Json => println!(
            "{}",
            serde_json::to_string(&result).expect("reports serialise")
        ),
        ReportFormat::Csv => {
            // CSV carries one table: the one asked for, or the first.
            if let Some(table) = result.tables.first() {
                print!("{}", report::to_csv(table));
            }
        }
        ReportFormat::Table => print_report(app, &result),
    }
    Ok(())
}

fn cell_text(value: &Json) -> String {
    match value {
        Json::Null => String::new(),
        // Log values are untrusted: an escape sequence must not reach the terminal.
        Json::String(s) => s
            .chars()
            .map(|c| if c.is_control() { '?' } else { c })
            .collect(),
        Json::Number(n) => n.as_u64().map(number).unwrap_or_else(|| n.to_string()),
        other => other.to_string(),
    }
}

fn print_report(app: &App, result: &ReportResult) {
    println!(
        "{} for {}, {} to {} ({})",
        result.report, result.site, result.from, result.to, result.timezone
    );
    for table in &result.tables {
        println!("\n{}", table.name);
        print_report_table(table);
    }
    for note in &result.notes {
        app.note(&format!("note: {note}"));
    }
}

fn print_report_table(table: &Table) {
    let headers: Vec<&str> = table.columns.iter().map(|c| c.key).collect();
    let rows: Vec<Vec<String>> = table
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .zip(&table.columns)
                // An AS number is an identifier, not a quantity: no separators.
                .map(|(value, column)| match (column.key, value) {
                    ("asn", Json::Number(n)) => n.to_string(),
                    _ => cell_text(value),
                })
                .collect()
        })
        .collect();
    if rows.is_empty() {
        println!("  (no rows)");
        return;
    }
    print_table(&headers, &rows);
    if !table.totals.is_empty() {
        let totals: Vec<String> = table
            .totals
            .iter()
            .map(|(key, value)| format!("{key} {}", cell_text(value)))
            .collect();
        println!("total: {}", totals.join(", "));
    }
    let shown = table.offset + table.rows.len() as u64;
    if table.total_rows > table.rows.len() as u64 {
        println!(
            "rows {}-{} of {} (use --limit and --offset for more)",
            table.offset + 1,
            shown,
            number(table.total_rows)
        );
    }
}

fn list_reports(app: &App) -> Result<()> {
    if app.json {
        let reports: Vec<Json> = report::REPORTS
            .iter()
            .map(|r| json!({ "name": r.name, "description": r.description, "tables": r.tables }))
            .collect();
        app.emit(&json!({
            "reports": reports,
            "parameters": ["from", "to", "status", "status-code", "path-prefix", "ip", "source", "country", "asn",
                           "bot", "verified", "referrer", "user-agent", "granularity", "table", "limit", "offset", "format"],
        }));
        return Ok(());
    }
    let rows: Vec<Vec<String>> = report::REPORTS
        .iter()
        .map(|r| {
            vec![
                r.name.to_owned(),
                r.tables.join(", "),
                r.description.to_owned(),
            ]
        })
        .collect();
    print_table(&["report", "tables", "description"], &rows);
    println!(
        "\nEvery report takes: --from DATE --to DATE --status CLASS --status-code CODE --path-prefix PREFIX --ip IP \
         --source TYPE --country ISO --asn N --bot NAME --verified STATE --referrer TEXT --user-agent TEXT --table NAME --limit N \
         --offset N --format json|table|csv"
    );
    Ok(())
}

fn db(app: &App, command: DbCommand) -> Result<()> {
    match command {
        DbCommand::Stats { slug: Some(slug) } => {
            let store = app.open(&slug, Mode::ReadOnly)?;
            let stats = maintain::stats(&app.dir, &store)?;
            if app.json {
                app.emit(&serde_json::to_value(&stats).expect("stats serialise"));
            } else {
                print_stats(&stats);
            }
        }
        DbCommand::Stats { slug: None } => {
            let all = all_stats(app)?;
            if app.json {
                let total: u64 = all.iter().filter_map(|s| s["size_bytes"].as_u64()).sum();
                app.emit(&json!({ "sites": all, "total_size_bytes": total }));
            } else {
                print_stats_list(&all);
            }
        }
        DbCommand::Files { slug } => {
            let store = app.open(&slug, Mode::ReadOnly)?;
            let files = maintain::files(&store)?;
            if app.json {
                app.emit(&json!({ "site": slug, "files": files }));
            } else if files.is_empty() {
                println!("No files ingested yet.");
            } else {
                let dash = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".to_owned());
                let rows: Vec<Vec<String>> = files
                    .iter()
                    .map(|f| {
                        vec![
                            f.file_id.to_string(),
                            number(f.rows_ok),
                            number(f.rows_bad),
                            number(f.rows_skipped),
                            dash(&f.first_ts),
                            dash(&f.last_ts),
                            bytes(f.size_bytes),
                            f.path.clone(),
                        ]
                    })
                    .collect();
                print_table(
                    &[
                        "id",
                        "rows",
                        "bad",
                        "skipped",
                        "first request",
                        "last request",
                        "size",
                        "path",
                    ],
                    &rows,
                );
            }
        }
        DbCommand::Prune {
            slug,
            before,
            rollups,
        } => {
            let store = app.open(&slug, Mode::ReadWrite)?;
            let deleted = maintain::prune(&store, before, rollups)?;
            if app.json {
                app.emit(&json!({ "site": slug, "rows_deleted": deleted, "before": before, "rollups_deleted": rollups }));
            } else {
                println!(
                    "Deleted {} rows before {before}{}. Run `logloupe db compact {slug}` to reclaim the space.",
                    number(deleted),
                    if rollups {
                        " and their rollups"
                    } else {
                        "; rollups kept"
                    }
                );
            }
        }
        DbCommand::DeleteFile { slug, file_id } => {
            let store = app.open(&slug, Mode::ReadWrite)?;
            let deleted = maintain::delete_file(&store, file_id)?;
            if app.json {
                app.emit(&json!({ "site": slug, "file_id": file_id, "rows_deleted": deleted }));
            } else {
                println!("Deleted file {file_id} and its {} rows.", number(deleted));
            }
        }
        DbCommand::Clear { slug } => {
            let store = app.open(&slug, Mode::ReadWrite)?;
            maintain::clear(&store)?;
            if app.json {
                app.emit(&json!({ "site": slug, "cleared": true }));
            } else {
                println!("Cleared all rows and rollups from '{slug}'. Settings kept.");
            }
        }
        DbCommand::Compact { slug } => {
            let result = maintain::compact(&app.dir, &slug, &app.opts)?;
            if app.json {
                app.emit(&json!({ "site": slug, "bytes_before": result.bytes_before, "bytes_after": result.bytes_after }));
            } else {
                println!(
                    "Compacted '{slug}': {} -> {}.",
                    bytes(result.bytes_before),
                    bytes(result.bytes_after)
                );
            }
        }
    }
    Ok(())
}

/// Refreshes the bot IP ranges when they are missing or older than a day.
/// Never fails the command: ingest works offline from whatever is cached.
fn refresh_ranges(app: &App) {
    if app.offline {
        return;
    }
    let Ok(registry) = Registry::load(app.dir.root()) else {
        return;
    };
    if !bot_ranges::any_stale(app.dir.root(), &registry) {
        return;
    }
    app.note("Updating bot IP ranges (skip with --offline)...");
    match bot_ranges::update_online(app.dir.root(), &registry) {
        Ok(outcomes) => {
            for outcome in outcomes.iter().filter(|o| !o.updated) {
                app.note(&format!(
                    "note: kept the previous ranges for {}: {}",
                    outcome.operator,
                    outcome.message.as_deref().unwrap_or("unknown error")
                ));
            }
        }
        Err(e) => app.note(&format!("note: could not update bot IP ranges: {e}")),
    }
}

fn print_range_status(app: &App) -> Result<()> {
    let registry = Registry::load(app.dir.root())?;
    let status = bot_ranges::status(app.dir.root(), &registry);
    if app.json {
        app.emit(&json!({ "operators": status }));
        return Ok(());
    }
    let rows: Vec<Vec<String>> = status
        .iter()
        .map(|s| {
            vec![
                s.name.clone(),
                number(s.prefixes as u64),
                number(s.retired as u64),
                s.tracked_since.map_or("-".to_owned(), |d| d.to_string()),
                s.fetched_at.clone().unwrap_or_else(|| "never".to_owned()),
                number(s.fetches as u64),
                if s.stale { "stale" } else { "fresh" }.to_owned(),
            ]
        })
        .collect();
    print_table(
        &[
            "operator",
            "prefixes",
            "retired",
            "tracked since",
            "last fetched (UTC)",
            "fetches",
            "state",
        ],
        &rows,
    );
    Ok(())
}

fn ranges(app: &App, command: RangesCommand) -> Result<()> {
    match command {
        RangesCommand::Status => print_range_status(app),
        RangesCommand::Update => {
            if app.offline {
                return Err(Error::Invalid(
                    "ranges update needs the network; remove --offline".into(),
                ));
            }
            let registry = Registry::load(app.dir.root())?;
            let outcomes = bot_ranges::update_online(app.dir.root(), &registry)?;
            if app.json {
                app.emit(&json!({ "operators": outcomes }));
                return Ok(());
            }
            for outcome in &outcomes {
                match &outcome.message {
                    None => println!(
                        "{}: {} prefixes ({} added, {} retired)",
                        outcome.operator,
                        number(outcome.prefixes as u64),
                        outcome.added,
                        outcome.retired
                    ),
                    Some(message) => {
                        println!("{}: kept the previous copy ({message})", outcome.operator)
                    }
                }
            }
            Ok(())
        }
    }
}

fn reenrich(app: &App, slug: &str) -> Result<()> {
    refresh_ranges(app);
    let store = app.open(slug, Mode::ReadWrite)?;
    let result = maintain::reenrich(&store, app.dns)?;
    if app.json {
        app.emit(&json!({ "site": slug, "result": result }));
    } else {
        println!(
            "Re-enriched '{slug}': {} user agents reclassified, {} bot addresses checked. \
             Bot requests now: {} verified, {} fake, {} unverifiable. {}",
            number(result.user_agents_changed),
            number(result.addresses_checked),
            number(result.verified),
            number(result.fake),
            number(result.unverifiable),
            if result.addresses_located > 0 {
                format!("Located {} addresses.", number(result.addresses_located))
            } else {
                "No geo database installed (run `logloupe geo update`).".to_owned()
            }
        );
        match result.dns {
            Some(dns) => println!(
                "Reverse DNS: {} addresses needed checking, {} looked up now, {} confirmed, {} got no answer{}.",
                number(dns.addresses),
                number(dns.looked_up),
                number(dns.confirmed),
                number(dns.failed),
                if dns.deferred > 0 {
                    format!(", {} left for the next run", number(dns.deferred))
                } else {
                    String::new()
                }
            ),
            None => app.note("Reverse DNS checks were not run. Add --dns to verify bots that publish no IP list."),
        }
    }
    Ok(())
}

fn geo(app: &App, command: GeoCommand) -> Result<()> {
    let status = match command {
        GeoCommand::Status => logloupe_core::geo::status(app.dir.root()),
        GeoCommand::Update => {
            if app.offline {
                return Err(Error::Invalid(
                    "geo update needs the network; remove --offline".into(),
                ));
            }
            app.note("Downloading DB-IP Lite country and ASN databases...");
            logloupe_core::geo::update_dbip(app.dir.root())?
        }
    };
    if app.json {
        app.emit(&serde_json::to_value(&status).expect("status serialises"));
        return Ok(());
    }
    let rows: Vec<Vec<String>> = status
        .files
        .iter()
        .map(|f| {
            vec![
                f.kind.to_owned(),
                f.provider.unwrap_or("not installed").to_owned(),
                f.built_at.clone().unwrap_or_else(|| "-".to_owned()),
                f.size_bytes.map(bytes).unwrap_or_else(|| "-".to_owned()),
                if !f.present {
                    "missing"
                } else if f.stale {
                    "stale"
                } else {
                    "fresh"
                }
                .to_owned(),
            ]
        })
        .collect();
    print_table(
        &["database", "provider", "built (UTC)", "size", "state"],
        &rows,
    );
    if let Some(attribution) = status.attribution {
        println!("\n{attribution}");
    }
    if status.files.iter().any(|f| !f.present) {
        println!(
            "\nRun `logloupe geo update` to download DB-IP Lite, or put your own GeoLite2-Country.mmdb and \
             GeoLite2-ASN.mmdb in {}.",
            logloupe_core::geo::geo_dir(app.dir.root()).display()
        );
    }
    Ok(())
}

fn blocklist(app: &App, args: BlocklistArgs) -> Result<()> {
    use logloupe_core::report::blocklist::{By, Style, collect, format};
    let (by, style): (By, Style) = (args.by.parse()?, args.style.parse()?);
    let params = ReportParams {
        limit: args.limit,
        ..args.filter.params()
    };
    let store = app.open(&args.slug, Mode::ReadOnly)?;
    let entries = collect(&store, &params, by, args.min_hits)?;
    if app.json {
        app.emit(
            &json!({ "site": args.slug, "entries": entries, "text": format(&entries, style)? }),
        );
        return Ok(());
    }
    if entries.is_empty() {
        app.note(&format!(
            "Nothing made {} or more requests with these filters. Lower --min-hits to see more.",
            args.min_hits
        ));
        return Ok(());
    }
    print!("{}", format(&entries, style)?);
    app.note(&format!(
        "{} entries. Verified bots and the server's own addresses are left out. Check the list before \
         using it: it is the busiest remaining traffic, not a judgement of who is hostile.",
        entries.len()
    ));
    Ok(())
}
