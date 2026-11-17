<script setup lang="ts">
// Ingest: drop or pick log files, or fetch them from a server, watch progress,
// and see what happened to each.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { open } from '@tauri-apps/plugin-dialog'
import { api, errorCode, errorMessage, inTauri, type FetchProgress, type FileReport, type IngestProgress, type RangeState } from '../api'
import { loadSites, refreshReports, state } from '../state'
import { fileName, formatNumber } from '../format'
import RemoteFetch from '../components/RemoteFetch.vue'

type Overlap = 'skip' | 'allow'

const toast = useToast()

const queued = ref<string[]>([])
const force = ref(false)
const offline = ref(localStorage.getItem('logloupe.offline') === 'true')
const running = ref(false)
const progress = ref<IngestProgress | FetchProgress | null>(null)
const results = ref<FileReport[]>([])
const notes = ref<string[]>([])
const error = ref('')
const errorTitle = ref('')
/** Set after an overlap error: runs the same thing again with a policy. */
const retry = ref<((overlap: Overlap) => void) | null>(null)
const dragging = ref(false)
/** The bot IP ranges are being downloaded before the run proper starts. */
const refreshingRanges = ref(false)
/** What happened to the bot IP ranges before the last run. */
const rangeState = ref<RangeState | null>(null)
const rangeText: Record<RangeState, string> = {
  updated: 'Bot IP ranges were updated before this run.',
  fresh: 'Bot IP ranges were already up to date (fetched within the last day), so they were not downloaded again.',
  skipped: 'Working offline: bot IP ranges were not refreshed.',
}
const unlisten: UnlistenFn[] = []

const percent = computed(() => {
  const p = progress.value
  if (!p || p.bytes_total === 0) return 0
  return Math.min(100, Math.round((p.bytes_done / p.bytes_total) * 100))
})

/** What is happening now, for the progress panel. */
const progressLabel = computed(() => {
  const p = progress.value
  if (!p) return refreshingRanges.value ? 'Updating bot IP ranges…' : 'Starting…'
  if (!('phase' in p)) return fileName(p.path)
  const doing = p.phase === 'download' ? 'Downloading' : 'Ingesting'
  return `File ${p.file} of ${p.files} · ${doing} ${fileName(p.path)}`
})
const showRows = computed(() => {
  const p = progress.value
  return !!p && (!('phase' in p) || p.phase === 'ingest')
})

function add(paths: string[]) {
  queued.value = [...new Set([...queued.value, ...paths])]
}

async function pick() {
  if (!inTauri) {
    toast.add({ title: 'Choosing files only works inside the app', color: 'warning' })
    return
  }
  const picked = await open({ multiple: true, title: 'Choose access logs' })
  if (picked) add(Array.isArray(picked) ? picked : [picked])
}

interface Done {
  files: FileReport[]
  notes: string[]
  ranges: RangeState
  /** The success message. */
  title: string
}

/** Runs an ingest or a fetch with the shared progress, results and errors. */
async function run(what: 'Ingest' | 'Fetch', again: (overlap: Overlap) => void, work: () => Promise<Done>) {
  running.value = true
  state.busy = true
  error.value = ''
  errorTitle.value = `${what} stopped`
  retry.value = null
  results.value = []
  notes.value = []
  progress.value = null
  refreshingRanges.value = false
  rangeState.value = null
  localStorage.setItem('logloupe.offline', String(offline.value))
  try {
    const done = await work()
    results.value = done.files
    notes.value = done.notes
    rangeState.value = done.ranges
    toast.add({ title: done.title, color: 'success' })
  } catch (e) {
    if (errorCode(e) === 'overlap') retry.value = again
    error.value = errorCode(e) === 'cancelled' ? `${what} cancelled. Nothing from the file in progress was kept.` : errorMessage(e)
  } finally {
    running.value = false
    state.busy = false
    progress.value = null
    await loadSites()
    refreshReports()
  }
}

const rowsAdded = (files: FileReport[]) => formatNumber(files.reduce((sum, f) => sum + f.rows_ok, 0))

async function start(overlap: Overlap | null = null) {
  if (!state.slug || queued.value.length === 0) return
  const slug = state.slug
  await run('Ingest', start, async () => {
    const outcome = await api.ingestFiles(slug, queued.value, { force: force.value, overlap, dns: state.dns && !offline.value }, offline.value)
    queued.value = []
    return { files: outcome.files, notes: outcome.range_notes, ranges: outcome.ranges, title: `Added ${rowsAdded(outcome.files)} rows` }
  })
}

async function startFetch(days: number | null, overlap: Overlap | null = null) {
  if (!state.slug || running.value) return
  const slug = state.slug
  await run('Fetch', (policy) => startFetch(days, policy), async () => {
    const { report, range_notes, ranges } = await api.fetchRemote(slug, { days, ingest: { overlap, dns: state.dns && !offline.value } }, offline.value)
    const title = report.files.length
      ? `Fetched ${report.files.length} file(s), added ${rowsAdded(report.files)} rows`
      : `Nothing new on ${report.host}`
    return { files: report.files, notes: [...range_notes, ...report.notes], ranges, title }
  })
}

const statusLabel: Record<FileReport['status'], string> = {
  ingested: 'Ingested',
  resumed: 'Resumed',
  unchanged: 'Already ingested',
  empty: 'Empty',
}
const statusColour: Record<FileReport['status'], 'success' | 'info' | 'neutral'> = {
  ingested: 'success',
  resumed: 'info',
  unchanged: 'neutral',
  empty: 'neutral',
}

onMounted(async () => {
  if (!inTauri) return
  unlisten.push(await listen<IngestProgress>('ingest-progress', (event) => (progress.value = event.payload)))
  unlisten.push(await listen<FetchProgress>('fetch-progress', (event) => (progress.value = event.payload)))
  unlisten.push(await listen('ranges-refresh', () => (refreshingRanges.value = true)))
  unlisten.push(
    await getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === 'over' || event.payload.type === 'enter') dragging.value = true
      else if (event.payload.type === 'leave') dragging.value = false
      else if (event.payload.type === 'drop') {
        dragging.value = false
        if (!running.value) add(event.payload.paths)
      }
    }),
  )
})

onBeforeUnmount(() => unlisten.forEach((stop) => stop()))
</script>

<template>
  <div class="space-y-4">
    <div>
      <h1 class="text-xl font-semibold text-highlighted">Ingest logs</h1>
      <p class="mt-0.5 text-sm text-muted">
        Add access logs to <span class="font-medium text-highlighted">{{ state.slug || 'a site' }}</span> from this
        computer or straight from your server. Plain and gzip files both work. A file you have already added is skipped,
        and a log that has grown only adds its new lines.
      </p>
    </div>

    <UAlert
      v-if="!state.slug"
      color="warning"
      variant="subtle"
      icon="i-lucide-triangle-alert"
      title="Add a site first"
      description="Logs are ingested into a site. Create one on the Sites screen."
    />

    <template v-else>
      <div
        class="rounded-lg border-2 border-dashed px-6 py-10 text-center transition-colors"
        :class="dragging ? 'border-primary bg-primary/5' : 'border-default'"
      >
        <UIcon name="i-lucide-file-down" class="mx-auto size-8 text-muted" />
        <p class="mt-2 font-medium text-highlighted">Drop log files here</p>
        <p class="mt-0.5 text-sm text-muted">or</p>
        <UButton class="mt-2" label="Choose files" color="neutral" variant="outline" :disabled="running" @click="pick" />
      </div>

      <section v-if="queued.length" class="rounded-lg border border-default bg-default">
        <header class="flex items-center justify-between border-b border-default px-4 py-2.5">
          <h2 class="text-sm font-semibold text-highlighted">{{ queued.length }} file(s) ready</h2>
          <UButton label="Clear" size="xs" color="neutral" variant="ghost" :disabled="running" @click="queued = []" />
        </header>
        <ul class="max-h-48 overflow-auto px-4 py-2 text-sm">
          <li v-for="path in queued" :key="path" class="truncate py-0.5 text-muted" :title="path">{{ path }}</li>
        </ul>
        <footer class="flex flex-wrap items-center justify-between gap-4 border-t border-default px-4 py-3">
          <div class="flex flex-wrap gap-x-6 gap-y-2">
            <USwitch v-model="force" label="Replace files already ingested" :disabled="running" />
            <USwitch v-model="offline" label="Work offline (do not refresh bot IP ranges)" :disabled="running" />
          </div>
          <UButton icon="i-lucide-play" label="Start ingest" :loading="running" @click="start()" />
        </footer>
      </section>

      <RemoteFetch v-model:offline="offline" :running="running" @fetch="(days) => startFetch(days)" />

      <section v-if="running" class="rounded-lg border border-default bg-default p-4">
        <div class="flex items-center justify-between gap-4 text-sm">
          <span class="truncate text-highlighted">{{ progressLabel }}</span>
          <span class="shrink-0 tabular-nums text-muted">
            {{ percent }}%<template v-if="progress && showRows"> · {{ formatNumber(progress.rows) }} rows</template>
          </span>
        </div>
        <UProgress class="mt-2" :model-value="percent" />
        <div class="mt-3 flex justify-end">
          <UButton label="Cancel" size="sm" color="neutral" variant="outline" @click="api.cancelIngest()" />
        </div>
      </section>

      <UAlert v-if="error" color="error" variant="subtle" icon="i-lucide-triangle-alert" :title="errorTitle" :description="error" />
      <div v-if="retry && !running" class="flex flex-wrap items-center gap-2 text-sm">
        <span class="text-muted">This file covers a time range you already have. Choose what to do:</span>
        <UButton label="Skip lines already covered" size="sm" color="neutral" variant="outline" @click="retry('skip')" />
        <UButton label="Ingest everything" size="sm" color="neutral" variant="outline" @click="retry('allow')" />
      </div>

      <UAlert
        v-for="note in notes"
        :key="note"
        color="warning"
        variant="subtle"
        icon="i-lucide-info"
        :title="note"
      />

      <p v-if="rangeState && !running" class="flex items-center gap-2 text-sm text-muted">
        <UIcon :name="rangeState === 'skipped' ? 'i-lucide-cloud-off' : 'i-lucide-shield-check'" class="size-4 shrink-0" />
        {{ rangeText[rangeState] }}
      </p>

      <section v-if="results.length" class="rounded-lg border border-default bg-default">
        <header class="border-b border-default px-4 py-2.5">
          <h2 class="text-sm font-semibold text-highlighted">Results</h2>
        </header>
        <div class="divide-y divide-default">
          <div v-for="file in results" :key="file.path" class="px-4 py-3 text-sm">
            <div class="flex items-center justify-between gap-4">
              <span class="truncate font-medium text-highlighted" :title="file.path">{{ fileName(file.path) }}</span>
              <UBadge :label="statusLabel[file.status]" :color="statusColour[file.status]" variant="subtle" size="sm" />
            </div>
            <p v-if="file.status === 'ingested' || file.status === 'resumed'" class="mt-1 tabular-nums text-muted">
              {{ formatNumber(file.rows_ok) }} rows added · {{ formatNumber(file.rows_bad) }} bad ·
              {{ formatNumber(file.rows_skipped) }} skipped · {{ file.format }}{{ file.gzip ? ', gzip' : '' }} ·
              {{ file.seconds.toFixed(1) }}s
            </p>
            <div v-if="file.bad_samples.length" class="mt-2 rounded-md bg-elevated p-2">
              <p class="text-xs font-medium text-muted">First lines that could not be parsed</p>
              <p v-for="(bad, i) in file.bad_samples.slice(0, 5)" :key="i" class="mt-1 truncate font-mono text-xs text-muted" :title="bad.line">
                {{ bad.reason }}: {{ bad.line }}
              </p>
            </div>
          </div>
        </div>
      </section>
    </template>
  </div>
</template>
