<script setup lang="ts">
// Generic report screen: runs one report for the selected site and shows each
// of its tables, with a chart on top where the report has a time series.
import { computed, ref, watch } from 'vue'
import { save } from '@tauri-apps/plugin-dialog'
import { api, errorCode, errorMessage, inTauri, type ReportResult, type ReportTable } from '../api'
import { currentSite, reportParams, state } from '../state'
import DataTable from '../components/DataTable.vue'
import SeriesChart from '../components/SeriesChart.vue'

const props = defineProps<{ report: string; title: string; description: string }>()
const toast = useToast()

const result = ref<ReportResult | null>(null)
const loading = ref(false)
const paging = ref<string | null>(null)
const error = ref('')
const granularityChoice = ref<'auto' | 'day' | 'hour'>('auto')

/** Days covered by the date range, or by the site's data when no range is set. */
const spanDays = computed(() => {
  const stats = currentSite.value?.stats
  const from = state.from || stats?.first_ts?.slice(0, 10)
  const to = state.to || stats?.last_ts?.slice(0, 10)
  if (!from || !to) return 0
  return Math.round((Date.parse(to) - Date.parse(from)) / 86_400_000) + 1
})

// A day or two of data makes a useless daily chart, so plot it by the hour.
const granularity = computed<'day' | 'hour'>(() =>
  granularityChoice.value === 'auto' ? (spanDays.value <= 3 ? 'hour' : 'day') : granularityChoice.value,
)
let run = 0

const isOverview = computed(() => props.report === 'overview')
// Reports with a time series load it whole, so the chart is not one page of it.
const hasSeries = computed(() => ['overview', 'bots', 'status-codes', 'crawl', 'googlebot'].includes(props.report))

/** What to plot: the table and which of its columns become lines. */
const chart = computed<{ table: ReportTable; keys: string[] } | null>(() => {
  const series = result.value?.tables.find((t) => t.name === 'series')
  if (!series) return null
  const keys = series.columns.map((c) => c.key)
  if (isOverview.value) {
    return series.rows.length > 1 ? { table: series, keys: ['requests', 'bot', 'human', 'pages'] } : null
  }
  // A long series (day, group, value) becomes one line per group, busiest first.
  if (keys.length !== 3) return null
  const totals = new Map<string, number>()
  const byDay = new Map<string, Map<string, number>>()
  for (const [day, group, value] of series.rows) {
    const name = String(group ?? 'other')
    const amount = Number(value ?? 0)
    totals.set(name, (totals.get(name) ?? 0) + amount)
    const dayKey = String(day)
    if (!byDay.has(dayKey)) byDay.set(dayKey, new Map())
    byDay.get(dayKey)!.set(name, amount)
  }
  if (byDay.size < 2) return null
  const groups = [...totals.entries()].sort((a, b) => b[1] - a[1]).slice(0, 5).map(([name]) => name)
  const rows = [...byDay.keys()].sort().map((day) => [day, ...groups.map((g) => byDay.get(day)!.get(g) ?? 0)])
  return {
    table: {
      ...series,
      columns: [{ key: 'bucket', type: 'date' }, ...groups.map((key) => ({ key, type: 'integer' as const }))],
      rows,
    },
    keys: groups,
  }
})

function params(extra = {}) {
  return reportParams({
    // The overview chart needs the whole series, not one page of it.
    limit: hasSeries.value ? 2000 : 50,
    granularity: hasSeries.value ? granularity.value : null,
    ...extra,
  })
}

async function load() {
  if (!state.slug) return
  const mine = ++run
  loading.value = true
  error.value = ''
  try {
    const data = await api.runReport(state.slug, props.report, params())
    if (mine === run) result.value = data
  } catch (e) {
    if (mine !== run) return
    result.value = null
    error.value =
      errorCode(e) === 'site_locked'
        ? 'This site is busy (an ingest or clean-up is running). The report will load when it finishes.'
        : errorMessage(e)
  } finally {
    if (mine === run) loading.value = false
  }
}

async function page(table: string, offset: number) {
  if (!result.value) return
  paging.value = table
  try {
    const data = await api.runReport(state.slug, props.report, params({ table, offset }))
    const fresh = data.tables[0]
    if (fresh) {
      result.value.tables = result.value.tables.map((t) => (t.name === table ? fresh : t))
    }
  } catch (e) {
    toast.add({ title: 'Could not load the page', description: errorMessage(e), color: 'error' })
  } finally {
    paging.value = null
  }
}

async function exportCsv(table: string) {
  if (!inTauri) {
    toast.add({ title: 'CSV export only works inside the app', color: 'warning' })
    return
  }
  const path = await save({
    defaultPath: `${state.slug}-${props.report}-${table}.csv`,
    filters: [{ name: 'CSV', extensions: ['csv'] }],
  })
  if (!path) return
  try {
    const rows = await api.exportCsv(state.slug, props.report, params(), table, path)
    toast.add({ title: `Exported ${rows.toLocaleString()} rows`, description: path, color: 'success' })
  } catch (e) {
    toast.add({ title: 'Export failed', description: errorMessage(e), color: 'error' })
  }
}

watch(() => [props.report, state.slug, state.revision, granularity.value], load, { immediate: true })
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-start justify-between gap-4">
      <div>
        <h1 class="text-xl font-semibold text-highlighted">{{ title }}</h1>
        <p class="mt-0.5 text-sm text-muted">{{ description }}</p>
        <p v-if="result" class="mt-1 text-xs text-muted">
          {{ result.from }} to {{ result.to }} · times in {{ result.timezone }}
          <template v-if="result.coverage.rollup_days > 0">
            · {{ result.coverage.rollup_days }} day(s) from rollups
          </template>
        </p>
      </div>
      <div class="flex items-center gap-2">
        <USelect
          v-if="hasSeries"
          v-model="granularityChoice"
          :items="[
            { label: `Auto (per ${granularity})`, value: 'auto' },
            { label: 'Per day', value: 'day' },
            { label: 'Per hour', value: 'hour' },
          ]"
          size="sm"
          class="w-40"
          aria-label="Chart interval"
        />
        <UButton
          v-if="report !== 'requests'"
          icon="i-lucide-list"
          color="neutral"
          variant="outline"
          size="sm"
          label="View requests"
          title="See the individual requests behind the current filters"
          @click="state.view = 'requests'"
        />
        <UButton
          icon="i-lucide-refresh-cw"
          color="neutral"
          variant="outline"
          size="sm"
          label="Refresh"
          :loading="loading"
          @click="load"
        />
      </div>
    </div>

    <UAlert v-if="error" color="error" variant="subtle" icon="i-lucide-triangle-alert" :title="error" />

    <div v-if="loading && !result" class="py-16 text-center text-sm text-muted">Running report…</div>

    <template v-if="result">
      <section v-if="chart" class="rounded-lg border border-default bg-default p-4">
        <SeriesChart :table="chart.table" :series="chart.keys" />
      </section>

      <DataTable
        v-for="table in result.tables"
        :key="table.name"
        :table="table"
        :loading="paging === table.name || loading"
        @page="(offset) => page(table.name, offset)"
        @export="exportCsv(table.name)"
      />

      <ul class="space-y-1 text-xs text-muted">
        <li class="flex gap-2">
          <UIcon name="i-lucide-mouse-pointer-click" class="mt-0.5 size-3.5 shrink-0" />
          <span>Click an address, path, referrer, user agent or bot in any table to show only that in every report.</span>
        </li>
        <li v-for="note in result.notes" :key="note" class="flex gap-2">
          <UIcon name="i-lucide-info" class="mt-0.5 size-3.5 shrink-0" />
          <span>{{ note }}</span>
        </li>
      </ul>
    </template>
  </div>
</template>
