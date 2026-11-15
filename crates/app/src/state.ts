// Shared app state: the site list, the selected site, and the date range and
// filters every report uses.

import { computed, reactive } from 'vue'
import { api, type Filters, type ReportParams, type SiteEntry } from './api'

export const state = reactive({
  sites: [] as SiteEntry[],
  sitesLoaded: false,
  slug: localStorage.getItem('loga.site') ?? '',
  from: '',
  to: '',
  bot: '',
  statusClass: '',
  pathPrefix: '',
  ip: '',
  source: '',
  referrer: '',
  userAgent: '',
  country: '',
  asn: '',
  /** Bumped to make open reports reload. */
  revision: 0,
  /** An ingest or other long write is running; reports cannot open the site. */
  busy: false,
})

export const currentSite = computed(() => state.sites.find((s) => s.slug === state.slug) ?? null)

export async function loadSites(): Promise<void> {
  state.sites = await api.listSites()
  state.sitesLoaded = true
  if (!state.sites.some((s) => s.slug === state.slug)) {
    state.slug = state.sites[0]?.slug ?? ''
  }
}

export function selectSite(slug: string): void {
  state.slug = slug
  localStorage.setItem('loga.site', slug)
  state.revision++
}

export function refreshReports(): void {
  state.revision++
}

export function clearFilters(): void {
  state.from = ''
  state.to = ''
  state.bot = ''
  state.statusClass = ''
  state.pathPrefix = ''
  state.ip = ''
  state.source = ''
  state.referrer = ''
  state.userAgent = ''
  state.country = ''
  state.asn = ''
  state.revision++
}

export const hasFilters = computed(
  () =>
    !!(
      state.from ||
      state.to ||
      state.bot ||
      state.statusClass ||
      state.pathPrefix ||
      state.ip ||
      state.source ||
      state.referrer ||
      state.userAgent ||
      state.country ||
      state.asn
    ),
)

/** The date range and filters as report parameters. */
export function reportParams(extra: Partial<ReportParams> = {}): ReportParams {
  const filters: Filters = {}
  if (state.bot.trim()) filters.bot = state.bot.trim()
  if (state.statusClass) filters.status_class = Number(state.statusClass)
  if (state.pathPrefix.trim()) filters.path_prefix = state.pathPrefix.trim()
  if (state.ip.trim()) filters.ip = state.ip.trim()
  if (state.source) filters.source_type = state.source
  if (state.referrer.trim()) filters.referrer = state.referrer.trim()
  if (state.userAgent.trim()) filters.user_agent = state.userAgent.trim()
  if (state.country) filters.country = state.country
  if (state.asn) filters.asn = Number(state.asn)
  return {
    from: state.from || null,
    to: state.to || null,
    filters,
    limit: 50,
    offset: 0,
    ...extra,
  }
}

/** Columns whose values can be turned into a filter by clicking a cell. */
const cellFilters: Record<string, (value: string) => void> = {
  ip: (v) => (state.ip = v),
  network: (v) => (state.ip = v),
  referrer: (v) => (state.referrer = v === '(none)' ? v : `=${v}`),
  referer: (v) => (state.referrer = `=${v}`),
  referrer_host: (v) => (state.referrer = v === '(none)' ? v : `//${v}`),
  referer_host: (v) => (state.referrer = `//${v}`),
  user_agent: (v) => (state.userAgent = v === '(none)' ? v : `=${v}`),
  path: (v) => (state.pathPrefix = v),
  bot: (v) => (state.bot = v),
  claimed_bot: (v) => (state.bot = v),
  source_type: (v) => (state.source = v),
  country: (v) => (state.country = v === '(unknown)' ? '' : v),
  asn: (v) => (state.asn = v),
}

export function canFilterBy(column: string): boolean {
  return column in cellFilters
}

/** Narrows every report to rows with this value, and reloads them. */
export function filterBy(column: string, value: string): void {
  cellFilters[column]?.(value)
  state.revision++
}

export type FilterKey =
  | 'from'
  | 'to'
  | 'bot'
  | 'statusClass'
  | 'pathPrefix'
  | 'ip'
  | 'source'
  | 'referrer'
  | 'userAgent'
  | 'country'
  | 'asn'

export interface ActiveFilter {
  key: FilterKey
  label: string
}

/** `(none)`, `=exact` or `contains` text, in words. */
function describeText(noun: string, value: string): string {
  if (value === '(none)') return `No ${noun}`
  const name = noun.charAt(0).toUpperCase() + noun.slice(1)
  return value.startsWith('=') ? `${name} is ${value.slice(1)}` : `${name} contains ${value}`
}

/** Everything currently narrowing the reports, in plain words. */
export const activeFilters = computed<ActiveFilter[]>(() => {
  const list: ActiveFilter[] = []
  if (state.from) list.push({ key: 'from', label: `From ${state.from}` })
  if (state.to) list.push({ key: 'to', label: `To ${state.to}` })
  if (state.statusClass) list.push({ key: 'statusClass', label: `Status ${state.statusClass}xx` })
  if (state.source) list.push({ key: 'source', label: `Source is ${state.source}` })
  if (state.pathPrefix) list.push({ key: 'pathPrefix', label: `Path starts with ${state.pathPrefix}` })
  if (state.ip) list.push({ key: 'ip', label: `IP ${state.ip}` })
  if (state.bot) list.push({ key: 'bot', label: `Bot is ${state.bot}` })
  if (state.referrer) list.push({ key: 'referrer', label: describeText('referrer', state.referrer) })
  if (state.userAgent) list.push({ key: 'userAgent', label: describeText('user agent', state.userAgent) })
  if (state.country) list.push({ key: 'country', label: `Country is ${state.country}` })
  if (state.asn) list.push({ key: 'asn', label: `Network is AS${state.asn}` })
  return list
})

export function removeFilter(key: FilterKey): void {
  state[key] = ''
  // Date changes reload the reports through the filter bar's own watcher.
  if (key !== 'from' && key !== 'to') state.revision++
}
