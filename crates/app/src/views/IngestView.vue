<script setup lang="ts">
// Ingest: drop or pick log files, watch progress, see what happened to each.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { open } from '@tauri-apps/plugin-dialog'
import { api, errorCode, errorMessage, inTauri, type FileReport, type IngestProgress } from '../api'
import { loadSites, refreshReports, state } from '../state'
import { fileName, formatNumber } from '../format'

const toast = useToast()

const queued = ref<string[]>([])
const force = ref(false)
const offline = ref(localStorage.getItem('logloupe.offline') === 'true')
const running = ref(false)
const progress = ref<IngestProgress | null>(null)
const results = ref<FileReport[]>([])
const notes = ref<string[]>([])
const error = ref('')
const overlapError = ref(false)
const dragging = ref(false)
const unlisten: UnlistenFn[] = []

const percent = computed(() => {
  const p = progress.value
  if (!p || p.bytes_total === 0) return 0
  return Math.min(100, Math.round((p.bytes_done / p.bytes_total) * 100))
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

async function start(overlap: 'skip' | 'allow' | null = null) {
  if (!state.slug || queued.value.length === 0) return
  running.value = true
  state.busy = true
  error.value = ''
  overlapError.value = false
  results.value = []
  notes.value = []
  progress.value = null
  localStorage.setItem('logloupe.offline', String(offline.value))
  try {
    const outcome = await api.ingestFiles(state.slug, queued.value, { force: force.value, overlap, dns: state.dns && !offline.value }, offline.value)
    results.value = outcome.files
    notes.value = outcome.range_notes
    queued.value = []
    const added = outcome.files.reduce((sum, f) => sum + f.rows_ok, 0)
    toast.add({ title: `Added ${formatNumber(added)} rows`, color: 'success' })
  } catch (e) {
    overlapError.value = errorCode(e) === 'overlap'
    error.value = errorCode(e) === 'cancelled' ? 'Ingest cancelled. Nothing from the file in progress was kept.' : errorMessage(e)
  } finally {
    running.value = false
    state.busy = false
    progress.value = null
    await loadSites()
    refreshReports()
  }
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
        Add access logs to <span class="font-medium text-highlighted">{{ state.slug || 'a site' }}</span>. Plain and gzip
        files both work. A file you have already added is skipped, and a log that has grown only adds its new lines.
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

      <section v-if="running" class="rounded-lg border border-default bg-default p-4">
        <div class="flex items-center justify-between gap-4 text-sm">
          <span class="truncate text-highlighted">{{ progress ? fileName(progress.path) : 'Starting…' }}</span>
          <span class="shrink-0 tabular-nums text-muted">
            {{ percent }}%<template v-if="progress"> · {{ formatNumber(progress.rows) }} rows</template>
          </span>
        </div>
        <UProgress class="mt-2" :model-value="percent" />
        <div class="mt-3 flex justify-end">
          <UButton label="Cancel" size="sm" color="neutral" variant="outline" @click="api.cancelIngest()" />
        </div>
      </section>

      <UAlert v-if="error" color="error" variant="subtle" icon="i-lucide-triangle-alert" title="Ingest stopped" :description="error" />
      <div v-if="overlapError && queued.length" class="flex flex-wrap items-center gap-2 text-sm">
        <span class="text-muted">This file covers a time range you already have. Choose what to do:</span>
        <UButton label="Skip lines already covered" size="sm" color="neutral" variant="outline" @click="start('skip')" />
        <UButton label="Ingest everything" size="sm" color="neutral" variant="outline" @click="start('allow')" />
      </div>

      <UAlert
        v-for="note in notes"
        :key="note"
        color="warning"
        variant="subtle"
        icon="i-lucide-info"
        :title="note"
      />

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
