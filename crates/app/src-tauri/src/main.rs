//! The desktop app's Rust side: thin Tauri commands over `logloupe-core`. Every
//! command calls the same engine function as its CLI counterpart and returns
//! the same serialisable struct.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::NaiveDate;
use logloupe_core::bots::{Registry, ranges};
use logloupe_core::ingest::{self, FileReport, IngestOptions};
use logloupe_core::maintain::{
    self, CompactResult, IngestedFile, ReenrichResult, SettingsChange, SiteEntry, SiteStats,
};
use logloupe_core::report::{self, ReportParams, ReportResult};
use logloupe_core::store::{DataDir, Mode, OpenOptions, SettingsPatch, SiteSettings, Store};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

struct AppState {
    dir: DataDir,
    opts: OpenOptions,
    cancel: Arc<AtomicBool>,
}

/// What the frontend receives when a command fails. `code` is the same
/// stable string the CLI puts in its JSON error object.
#[derive(Debug, Serialize)]
struct CommandError {
    code: String,
    message: String,
}

impl From<logloupe_core::Error> for CommandError {
    fn from(error: logloupe_core::Error) -> Self {
        CommandError {
            code: error.code().to_owned(),
            message: error.to_string(),
        }
    }
}

type CommandResult<T> = Result<T, CommandError>;

/// Runs engine work off the UI thread.
async fn blocking<T, F>(work: F) -> CommandResult<T>
where
    F: FnOnce() -> logloupe_core::Result<T> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| CommandError {
            code: "internal_error".to_owned(),
            message: e.to_string(),
        })?
        .map_err(Into::into)
}

#[derive(Serialize)]
struct AppInfo {
    version: &'static str,
    data_dir: String,
}

#[tauri::command]
fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        data_dir: state.dir.root().to_string_lossy().into_owned(),
    }
}

/// A screen to open on at start-up, from the `LOGLOUPE_VIEW` environment
/// variable (a report name, or ingest, storage, sites, settings).
#[tauri::command]
fn initial_view() -> Option<String> {
    std::env::var("LOGLOUPE_VIEW")
        .ok()
        .filter(|v| !v.is_empty())
}

#[tauri::command]
async fn list_sites(state: State<'_, AppState>) -> CommandResult<Vec<SiteEntry>> {
    let (dir, opts) = (state.dir.clone(), state.opts.clone());
    blocking(move || maintain::all_stats(&dir, &opts)).await
}

#[tauri::command]
async fn add_site(
    state: State<'_, AppState>,
    slug: String,
    settings: SettingsPatch,
) -> CommandResult<SiteStats> {
    let (dir, opts) = (state.dir.clone(), state.opts.clone());
    blocking(move || {
        let mut site = SiteSettings::default();
        // A new site must name its hosts.
        SettingsPatch {
            hosts: Some(settings.hosts.clone().unwrap_or_default()),
            ..settings
        }
        .apply(&mut site)?;
        let store = Store::create(&dir, &slug, &site, &opts)?;
        maintain::stats(&dir, &store)
    })
    .await
}

#[tauri::command]
async fn update_site(
    state: State<'_, AppState>,
    slug: String,
    settings: SettingsPatch,
) -> CommandResult<SettingsChange> {
    let (dir, opts) = (state.dir.clone(), state.opts.clone());
    blocking(move || {
        let store = Store::open(&dir, &slug, Mode::ReadWrite, &opts)?;
        maintain::update_settings(&store, &settings)
    })
    .await
}

#[tauri::command]
async fn remove_site(state: State<'_, AppState>, slug: String) -> CommandResult<()> {
    let dir = state.dir.clone();
    blocking(move || dir.remove_site(&slug)).await
}

#[derive(Clone, Serialize)]
struct IngestProgress {
    path: String,
    bytes_done: u64,
    bytes_total: u64,
    rows: u64,
}

#[derive(Serialize)]
struct IngestOutcome {
    files: Vec<FileReport>,
    /// Problems refreshing the bot IP ranges before the ingest, if any.
    range_notes: Vec<String>,
}

#[tauri::command]
async fn ingest_files(
    app: AppHandle,
    state: State<'_, AppState>,
    slug: String,
    paths: Vec<PathBuf>,
    options: IngestOptions,
    offline: bool,
) -> CommandResult<IngestOutcome> {
    let (dir, opts) = (state.dir.clone(), state.opts.clone());
    let cancel = Arc::clone(&state.cancel);
    cancel.store(false, Ordering::Relaxed);
    blocking(move || {
        let mut range_notes = Vec::new();
        if !offline {
            let registry = Registry::load(dir.root())?;
            match ranges::refresh_if_stale(dir.root(), &registry) {
                Some(Ok(outcomes)) => {
                    range_notes.extend(outcomes.into_iter().filter(|o| !o.updated).map(|o| {
                        format!(
                            "Kept the previous IP ranges for {}: {}",
                            o.operator,
                            o.message.unwrap_or_default()
                        )
                    }))
                }
                Some(Err(e)) => range_notes.push(format!("Could not update bot IP ranges: {e}")),
                None => {}
            }
        }
        let mut store = Store::open(&dir, &slug, Mode::ReadWrite, &opts)?;
        let mut progress = |p: ingest::Progress<'_>| {
            let _ = app.emit(
                "ingest-progress",
                IngestProgress {
                    path: p.path.to_string_lossy().into_owned(),
                    bytes_done: p.bytes_done,
                    bytes_total: p.bytes_total,
                    rows: p.rows,
                },
            );
        };
        let files = ingest::ingest(&mut store, &paths, &options, &mut progress, &cancel)?;
        Ok(IngestOutcome { files, range_notes })
    })
    .await
}

#[tauri::command]
fn cancel_ingest(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::Relaxed);
}

#[derive(Serialize)]
struct ReportListing {
    name: &'static str,
    description: &'static str,
    tables: &'static [&'static str],
}

#[tauri::command]
fn list_reports() -> Vec<ReportListing> {
    report::REPORTS
        .iter()
        .map(|r| ReportListing {
            name: r.name,
            description: r.description,
            tables: r.tables,
        })
        .collect()
}

#[tauri::command]
async fn run_report(
    state: State<'_, AppState>,
    slug: String,
    name: String,
    params: ReportParams,
) -> CommandResult<ReportResult> {
    let (dir, opts) = (state.dir.clone(), state.opts.clone());
    blocking(move || {
        let store = Store::open(&dir, &slug, Mode::ReadOnly, &opts)?;
        report::run(&store, &name, &params)
    })
    .await
}

/// Writes one table of a report to a CSV file. Returns the number of rows written.
#[tauri::command]
async fn export_csv(
    state: State<'_, AppState>,
    slug: String,
    name: String,
    params: ReportParams,
    table: String,
    path: PathBuf,
) -> CommandResult<usize> {
    let (dir, opts) = (state.dir.clone(), state.opts.clone());
    blocking(move || {
        let store = Store::open(&dir, &slug, Mode::ReadOnly, &opts)?;
        let params = ReportParams {
            table: Some(table),
            limit: 5_000_000,
            offset: 0,
            ..params
        };
        let result = report::run(&store, &name, &params)?;
        let Some(table) = result.tables.first() else {
            return Ok(0);
        };
        // The path comes from the save dialog; only ever write a .csv file.
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("csv"))
        {
            return Err(logloupe_core::Error::Invalid(
                "exports must be saved as a .csv file".into(),
            ));
        }
        std::fs::write(&path, report::to_csv(table))?;
        Ok(table.rows.len())
    })
    .await
}

#[tauri::command]
async fn site_files(state: State<'_, AppState>, slug: String) -> CommandResult<Vec<IngestedFile>> {
    let (dir, opts) = (state.dir.clone(), state.opts.clone());
    blocking(move || maintain::files(&Store::open(&dir, &slug, Mode::ReadOnly, &opts)?)).await
}

#[tauri::command]
async fn prune_site(
    state: State<'_, AppState>,
    slug: String,
    before: NaiveDate,
    rollups: bool,
) -> CommandResult<u64> {
    let (dir, opts) = (state.dir.clone(), state.opts.clone());
    blocking(move || {
        maintain::prune(
            &Store::open(&dir, &slug, Mode::ReadWrite, &opts)?,
            before,
            rollups,
        )
    })
    .await
}

#[tauri::command]
async fn delete_file(state: State<'_, AppState>, slug: String, file_id: i32) -> CommandResult<u64> {
    let (dir, opts) = (state.dir.clone(), state.opts.clone());
    blocking(move || {
        maintain::delete_file(&Store::open(&dir, &slug, Mode::ReadWrite, &opts)?, file_id)
    })
    .await
}

#[tauri::command]
async fn clear_site(state: State<'_, AppState>, slug: String) -> CommandResult<()> {
    let (dir, opts) = (state.dir.clone(), state.opts.clone());
    blocking(move || maintain::clear(&Store::open(&dir, &slug, Mode::ReadWrite, &opts)?)).await
}

#[tauri::command]
async fn compact_site(state: State<'_, AppState>, slug: String) -> CommandResult<CompactResult> {
    let (dir, opts) = (state.dir.clone(), state.opts.clone());
    blocking(move || maintain::compact(&dir, &slug, &opts)).await
}

#[tauri::command]
async fn reenrich_site(
    state: State<'_, AppState>,
    slug: String,
    dns: bool,
) -> CommandResult<ReenrichResult> {
    let (dir, opts) = (state.dir.clone(), state.opts.clone());
    blocking(move || maintain::reenrich(&Store::open(&dir, &slug, Mode::ReadWrite, &opts)?, dns))
        .await
}

#[tauri::command]
async fn ranges_status(state: State<'_, AppState>) -> CommandResult<Vec<ranges::RangeStatus>> {
    let dir = state.dir.clone();
    blocking(move || Ok(ranges::status(dir.root(), &Registry::load(dir.root())?))).await
}

#[tauri::command]
async fn ranges_update(state: State<'_, AppState>) -> CommandResult<Vec<ranges::UpdateOutcome>> {
    let dir = state.dir.clone();
    blocking(move || ranges::update_online(dir.root(), &Registry::load(dir.root())?)).await
}

#[derive(Serialize)]
struct BlockList {
    text: String,
    entries: usize,
    /// Left out because they have verified bot traffic or are the server's own addresses.
    left_out: Vec<String>,
}

/// Formats addresses and networks as a block list. The engine drops any with
/// verified bot traffic and any local address, and refuses anything that is
/// not an address or network.
#[tauri::command]
async fn format_block_list(
    state: State<'_, AppState>,
    slug: String,
    entries: Vec<String>,
    style: report::blocklist::Style,
) -> CommandResult<BlockList> {
    let (dir, opts) = (state.dir.clone(), state.opts.clone());
    blocking(move || {
        let store = Store::open(&dir, &slug, Mode::ReadOnly, &opts)?;
        let (kept, left_out) = report::blocklist::protect(&store, &entries)?;
        Ok(BlockList {
            text: report::blocklist::format(&kept, style)?,
            entries: kept.len(),
            left_out,
        })
    })
    .await
}

#[tauri::command]
async fn geo_status(state: State<'_, AppState>) -> CommandResult<logloupe_core::geo::GeoStatus> {
    let dir = state.dir.clone();
    blocking(move || Ok(logloupe_core::geo::status(dir.root()))).await
}

#[tauri::command]
async fn geo_update(state: State<'_, AppState>) -> CommandResult<logloupe_core::geo::GeoStatus> {
    let dir = state.dir.clone();
    blocking(move || logloupe_core::geo::update_dbip(dir.root())).await
}

fn main() {
    let dir = DataDir::resolve(None).expect("a per-user data directory is available");
    tauri::Builder::default()
        // Must be registered first. Two copies would fight over the same site
        // databases, so starting the app again brings the open window forward.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            dir,
            opts: OpenOptions::default(),
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            initial_view,
            list_sites,
            add_site,
            update_site,
            remove_site,
            ingest_files,
            cancel_ingest,
            list_reports,
            run_report,
            export_csv,
            site_files,
            prune_site,
            delete_file,
            clear_site,
            compact_site,
            reenrich_site,
            ranges_status,
            ranges_update,
            geo_status,
            geo_update,
            format_block_list,
        ])
        .run(tauri::generate_context!())
        .expect("the app failed to start");
}
