// Canned data for working on the frontend in a plain browser (`pnpm dev`
// without Tauri). Never used inside the app.

import type { ReportResult, ReportTable, SiteEntry } from './api'

const site: SiteEntry = {
  slug: 'example',
  locked: false,
  size_bytes: 8_800_000,
  stats: {
    slug: 'example',
    hosts: ['example.com', 'www.example.com'],
    timezone: 'Europe/London',
    retention: 'forever',
    overlap: 'error',
    client_ip: 'auto',
    edge_cache: true,
    size_bytes: 8_800_000,
    rows: 96_000,
    first_ts: '2026-11-14T00:00:05Z',
    last_ts: '2026-11-15T18:30:00Z',
    files: 2,
    user_agents: 2_400,
    rollup_only_days: 0,
    pruned_before: null,
  },
}

const reports = [
  { name: 'overview', description: 'Requests, pages, bytes, unique IPs per day or hour.', tables: ['series', 'totals'] },
  { name: 'sources', description: 'Pages by source type and referrer.', tables: ['by_source_type', 'by_referrer_host'] },
  { name: 'bots', description: 'Registered bots.', tables: ['by_bot'] },
  { name: 'fake-bots', description: 'Fake bots.', tables: ['by_bot'] },
  { name: 'top-ips', description: 'Busiest addresses.', tables: ['by_ip'] },
  ...['crawl', 'crawled-urls', 'crawl-errors', 'crawl-waste', 'crawl-housekeeping', 'googlebot'].map((name) => ({
    name,
    description: 'Search engine crawling.',
    tables: ['by_bot'],
  })),
]

function seriesTable(): ReportTable {
  const rows = Array.from({ length: 14 }, (_, i) => {
    const requests = 40_000 + Math.round(Math.sin(i / 2) * 9_000) + i * 600
    return [
      `2026-11-${String(i + 1).padStart(2, '0')}`,
      requests,
      Math.round(requests * 0.06),
      requests * 41_000,
      Math.round(requests / 5),
      Math.round(requests * 0.45),
      Math.round(requests * 0.5),
      Math.round(requests * 0.44),
      Math.round(requests * 0.34),
      Math.round(requests * 0.22),
      i % 5,
    ]
  })
  return {
    name: 'series',
    columns: ['bucket', 'requests', 'pages', 'bytes', 'unique_ips', 'human', 'bot', 's2xx', 's3xx', 's4xx', 's5xx'].map(
      (key, i) => ({ key, type: i === 0 ? 'date' : 'integer' }),
    ),
    rows,
    totals: {},
    total_rows: rows.length,
    offset: 0,
    limit: 1000,
  }
}

function genericTable(name: string): ReportTable {
  const bots = ['Googlebot', 'Bingbot', 'GPTBot', 'ClaudeBot', 'Amazonbot', 'Applebot', 'PerplexityBot']
  return {
    name,
    columns: [
      { key: 'bot', type: 'string' },
      { key: 'kind', type: 'string' },
      { key: 'hits', type: 'integer' },
      { key: 'bytes', type: 'integer' },
      { key: 'verified', type: 'integer' },
      { key: 'fake', type: 'integer' },
      { key: 'unverifiable', type: 'integer' },
    ],
    rows: bots.map((bot, i) => [bot, i < 2 ? 'search' : 'ai-training', 9000 - i * 1100, (9000 - i * 1100) * 30_000, 8000 - i * 1000, i * 37, 0]),
    totals: { hits: 39_900, bytes: 1_197_000_000 },
    total_rows: 36,
    offset: 0,
    limit: 50,
  }
}

function ipTable(): ReportTable {
  return {
    name: 'by_ip',
    columns: [
      { key: 'ip', type: 'string' },
      { key: 'network', type: 'string' },
      { key: 'country', type: 'string' },
      { key: 'hits', type: 'integer' },
      { key: 'errors', type: 'integer' },
      { key: 'verify', type: 'string' },
    ],
    rows: Array.from({ length: 30 }, (_, i) => [
      `203.0.113.${i + 1}`,
      '203.0.113.0/24',
      i % 3 ? 'GB' : 'US',
      5000 - i * 140,
      i * 12,
      i === 2 ? 'verified' : null,
    ]),
    totals: { hits: 90_000, ips: 30 },
    total_rows: 30,
    offset: 0,
    limit: 50,
  }
}

function report(name: string, table?: string | null): ReportResult {
  const listing = reports.find((r) => r.name === name) ?? reports[0]
  const tables = listing.tables
    .filter((t) => !table || t === table)
    .map((t) => (t === 'series' ? seriesTable() : t === 'by_ip' ? ipTable() : genericTable(t)))
  return {
    report: name,
    site: 'example',
    from: '2026-11-01',
    to: '2026-11-14',
    timezone: 'Europe/London',
    generated_at: new Date().toISOString(),
    schema_version: 1,
    coverage: { raw_from: '2026-11-01', raw_to: '2026-11-14', rollup_days: 0 },
    tables,
    notes: ['Preview data: the page is not running inside the app.'],
  }
}

export async function mockInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  await new Promise((resolve) => setTimeout(resolve, 150))
  const data: Record<string, () => unknown> = {
    app_info: () => ({ version: '0.2.0', data_dir: '(browser preview)' }),
    list_timezones: () => ['UTC', ...Intl.supportedValuesOf('timeZone')],
    list_sites: () => [site],
    list_reports: () => reports,
    run_report: () => report(String(args?.name), (args?.params as { table?: string })?.table),
    site_files: () => [
      {
        file_id: 1, source: 'local', path: 'D:\\logs\\access.log', size_bytes: 14_500_000, format: 'combined',
        first_ts: '2026-11-14T00:00:05Z', last_ts: '2026-11-14T23:59:50Z', rows_ok: 52_000, rows_bad: 0,
        rows_skipped: 0, ingested_at: '2026-11-15T18:45:00Z',
      },
    ],
    get_remote: () => ({ host: 'web1', port: null, paths: ['/var/log/nginx/access.log*'] }),
    list_remote: () => ({
      host: 'web1',
      files: [
        { path: '/var/log/nginx/access.log.2.gz', size_bytes: 1_900_000, modified: 1_794_700_800, state: 'fetched' },
        { path: '/var/log/nginx/access.log.1', size_bytes: 14_500_000, modified: 1_794_787_200, state: 'changed' },
        { path: '/var/log/nginx/access.log', size_bytes: 6_200_000, modified: 1_794_850_000, state: 'new' },
      ],
      notes: [],
    }),
    ranges_status: () => [
      {
        operator: 'google', name: 'Google', sources: 1, prefixes: 317, retired: 0, tracked_since: '2026-11-15',
        fetched_at: '2026-11-15T19:01:51Z', fetches: 1, stale: false,
      },
      {
        operator: 'openai', name: 'OpenAI', sources: 4, prefixes: 290, retired: 0, tracked_since: '2026-11-15',
        fetched_at: '2026-11-15T19:01:51Z', fetches: 1, stale: false,
      },
    ],
  }
  const handler = data[command]
  if (!handler) {
    throw { code: 'preview', message: `"${command}" only works inside the app.` }
  }
  return handler() as T
}
