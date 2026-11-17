// Typed wrappers over the Tauri commands. Each maps to one engine function;
// the shapes match what the CLI prints with --json.

import { invoke } from '@tauri-apps/api/core'
import { mockInvoke } from './mock'

/** False when the page is opened in a plain browser (frontend development). */
export const inTauri = '__TAURI_INTERNALS__' in window

export interface CommandError {
  code: string
  message: string
}

export function errorMessage(error: unknown): string {
  if (error && typeof error === 'object' && 'message' in error) {
    return String((error as CommandError).message)
  }
  return String(error)
}

export function errorCode(error: unknown): string | undefined {
  if (error && typeof error === 'object' && 'code' in error) {
    return String((error as CommandError).code)
  }
  return undefined
}

function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return inTauri ? invoke<T>(command, args) : mockInvoke<T>(command, args)
}

export interface SiteStats {
  slug: string
  hosts: string[]
  timezone: string
  retention: string
  overlap: string
  client_ip: string
  edge_cache: boolean
  size_bytes: number
  rows: number
  first_ts: string | null
  last_ts: string | null
  files: number
  user_agents: number
  rollup_only_days: number
  pruned_before: string | null
}

export interface SiteEntry {
  slug: string
  locked: boolean
  size_bytes: number
  stats: SiteStats | null
}

export interface SettingsPatch {
  hosts?: string[]
  timezone?: string
  retention?: string
  overlap?: string
  client_ip?: string
  edge_cache?: boolean
}

export interface SettingsChange {
  hosts_changed: boolean
  stale_rollup_days: number
}

export type ColumnType = 'string' | 'integer' | 'float' | 'boolean' | 'date' | 'datetime'
export type Cell = string | number | boolean | null

export interface Column {
  key: string
  type: ColumnType
}

export interface ReportTable {
  name: string
  columns: Column[]
  rows: Cell[][]
  totals: Record<string, Cell>
  total_rows: number
  offset: number
  limit: number
}

export interface ReportResult {
  report: string
  site: string
  from: string
  to: string
  timezone: string
  generated_at: string
  schema_version: number
  coverage: { raw_from: string | null; raw_to: string | null; rollup_days: number }
  tables: ReportTable[]
  notes: string[]
}

export interface Filters {
  status_class?: number | null
  status?: number | null
  path_prefix?: string | null
  ip?: string | null
  source_type?: string | null
  country?: string | null
  asn?: number | null
  bot?: string | null
  verified?: string | null
  referrer?: string | null
  user_agent?: string | null
}

export interface ReportParams {
  from?: string | null
  to?: string | null
  filters?: Filters
  granularity?: 'hour' | 'day' | null
  threshold?: number | null
  table?: string | null
  limit?: number
  offset?: number
}

export interface ReportListing {
  name: string
  description: string
  tables: string[]
}

export interface BadLine {
  reason: string
  line: string
}

export interface FileReport {
  path: string
  status: 'ingested' | 'resumed' | 'unchanged' | 'empty'
  file_id: number | null
  format: string | null
  gzip: boolean
  size_bytes: number
  rows_ok: number
  rows_bad: number
  rows_skipped: number
  first_ts: number | null
  last_ts: number | null
  bad_samples: BadLine[]
  seconds: number
}

/** The server a site's logs are fetched from over SSH. */
export interface Remote {
  host: string
  port?: number | null
  paths: string[]
}

export interface RemoteFile {
  path: string
  size_bytes: number
  /** Unix seconds, UTC. */
  modified: number
  state: 'new' | 'changed' | 'fetched' | 'old'
}

export interface RemoteListing {
  host: string
  files: RemoteFile[]
  notes: string[]
}

export interface FetchOptions {
  days?: number | null
  ingest?: IngestOptions
}

export interface FetchReport {
  host: string
  files: FileReport[]
  already_fetched: number
  too_old: number
  bytes_downloaded: number
  notes: string[]
}

export interface FetchOutcome {
  report: FetchReport
  range_notes: string[]
  ranges: RangeState
}

/** What happened to the bot IP ranges before an ingest or a fetch. */
export type RangeState = 'updated' | 'fresh' | 'skipped'

export interface FetchProgress extends IngestProgress {
  phase: 'download' | 'ingest'
  /** Which file this is, counting from 1, and of how many. */
  file: number
  files: number
}

export interface IngestOutcome {
  files: FileReport[]
  range_notes: string[]
  ranges: RangeState
}

export interface IngestOptions {
  format?: string | null
  force?: boolean
  overlap?: 'error' | 'skip' | 'allow' | null
  dns?: boolean
}

export interface IngestProgress {
  path: string
  bytes_done: number
  bytes_total: number
  rows: number
}

export interface IngestedFile {
  file_id: number
  source: string
  path: string
  size_bytes: number
  format: string
  first_ts: string | null
  last_ts: string | null
  rows_ok: number
  rows_bad: number
  rows_skipped: number
  ingested_at: string | null
}

export interface RangeStatus {
  operator: string
  name: string
  sources: number
  prefixes: number
  retired: number
  tracked_since: string | null
  fetched_at: string | null
  fetches: number
  stale: boolean
}

export interface UpdateOutcome {
  operator: string
  updated: boolean
  prefixes: number
  added: number
  retired: number
  message: string | null
}

export interface GeoFileStatus {
  kind: 'country' | 'asn'
  present: boolean
  provider: string | null
  file: string | null
  size_bytes: number | null
  built_at: string | null
  stale: boolean
}

export interface GeoStatus {
  files: GeoFileStatus[]
  attribution: string | null
}

export interface ReenrichResult {
  user_agents_changed: number
  addresses_checked: number
  verified: number
  fake: number
  unverifiable: number
  addresses_located: number
  dns: { addresses: number; looked_up: number; failed: number; deferred: number; confirmed: number } | null
}

export const api = {
  initialView: () => call<string | null>('initial_view'),
  appInfo: () => call<{ version: string; data_dir: string }>('app_info'),
  listSites: () => call<SiteEntry[]>('list_sites'),
  addSite: (slug: string, settings: SettingsPatch) => call<SiteStats>('add_site', { slug, settings }),
  updateSite: (slug: string, settings: SettingsPatch) =>
    call<SettingsChange>('update_site', { slug, settings }),
  removeSite: (slug: string) => call<void>('remove_site', { slug }),
  ingestFiles: (slug: string, paths: string[], options: IngestOptions, offline: boolean) =>
    call<IngestOutcome>('ingest_files', { slug, paths, options, offline }),
  cancelIngest: () => call<void>('cancel_ingest'),
  getRemote: (slug: string) => call<Remote | null>('get_remote', { slug }),
  setRemote: (slug: string, remote: Remote) => call<Remote>('set_remote', { slug, remote }),
  removeRemote: (slug: string) => call<boolean>('remove_remote', { slug }),
  listRemote: (slug: string, days: number | null) => call<RemoteListing>('list_remote', { slug, days }),
  fetchRemote: (slug: string, options: FetchOptions, offline: boolean) =>
    call<FetchOutcome>('fetch_remote', { slug, options, offline }),
  listReports: () => call<ReportListing[]>('list_reports'),
  runReport: (slug: string, name: string, params: ReportParams) =>
    call<ReportResult>('run_report', { slug, name, params }),
  exportCsv: (slug: string, name: string, params: ReportParams, table: string, path: string) =>
    call<number>('export_csv', { slug, name, params, table, path }),
  siteFiles: (slug: string) => call<IngestedFile[]>('site_files', { slug }),
  pruneSite: (slug: string, before: string, rollups: boolean) =>
    call<number>('prune_site', { slug, before, rollups }),
  deleteFile: (slug: string, fileId: number) => call<number>('delete_file', { slug, fileId }),
  clearSite: (slug: string) => call<void>('clear_site', { slug }),
  compactSite: (slug: string) =>
    call<{ bytes_before: number; bytes_after: number }>('compact_site', { slug }),
  reenrichSite: (slug: string, dns: boolean) => call<ReenrichResult>('reenrich_site', { slug, dns }),
  rangesStatus: () => call<RangeStatus[]>('ranges_status'),
  rangesUpdate: () => call<UpdateOutcome[]>('ranges_update'),
  geoStatus: () => call<GeoStatus>('geo_status'),
  geoUpdate: () => call<GeoStatus>('geo_update'),
  formatBlockList: (slug: string, entries: string[], style: 'plain' | 'nginx' | 'apache') =>
    call<{ text: string; entries: number; left_out: string[] }>('format_block_list', { slug, entries, style }),
}
