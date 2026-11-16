<script setup lang="ts">
// Storage for the selected site: size, ingested files, prune, clear, compact,
// and re-running bot verification.
import { ref, watch } from 'vue'
import { api, errorCode, errorMessage, type IngestedFile } from '../api'
import { currentSite, loadSites, refreshReports, state } from '../state'
import { fileName, formatBytes, formatNumber, formatUtc } from '../format'

const toast = useToast()

const files = ref<IngestedFile[]>([])
const loadError = ref('')
const working = ref('')
const pruneBefore = ref('')
const pruneRollups = ref(false)

interface Confirmation {
  title: string
  description: string
  action: string
  run: () => Promise<string>
}
const confirming = ref<Confirmation | null>(null)

async function load() {
  loadError.value = ''
  files.value = []
  if (!state.slug) return
  try {
    files.value = await api.siteFiles(state.slug)
  } catch (e) {
    loadError.value =
      errorCode(e) === 'site_locked' ? 'This site is busy. Its files will show when the current job finishes.' : errorMessage(e)
  }
}

/** Runs a write on the site, then refreshes everything that depends on it. */
async function perform(name: string, run: () => Promise<string>) {
  working.value = name
  state.busy = true
  try {
    toast.add({ title: await run(), color: 'success' })
  } catch (e) {
    toast.add({ title: `${name} failed`, description: errorMessage(e), color: 'error' })
  } finally {
    working.value = ''
    state.busy = false
    confirming.value = null
    await loadSites()
    await load()
    refreshReports()
  }
}

function askDeleteFile(file: IngestedFile) {
  confirming.value = {
    title: `Delete ${fileName(file.path)}?`,
    description: `This removes the ${formatNumber(file.rows_ok)} rows that came from this file and rebuilds the daily rollups for its days. The log file itself is not touched.`,
    action: 'Delete rows',
    run: async () => `Deleted ${formatNumber(await api.deleteFile(state.slug, file.file_id))} rows`,
  }
}

function askPrune() {
  if (!pruneBefore.value) return
  const before = pruneBefore.value
  const rollups = pruneRollups.value
  confirming.value = {
    title: `Delete rows before ${before}?`,
    description: rollups
      ? 'This deletes the raw rows and the daily rollups for those days. Nothing about them will remain.'
      : 'This deletes the raw rows for those days. Daily rollups are kept, so the overview still covers them, but other reports will not.',
    action: 'Delete rows',
    run: async () => `Deleted ${formatNumber(await api.pruneSite(state.slug, before, rollups))} rows`,
  }
}

function askClear() {
  confirming.value = {
    title: `Clear all data from ${state.slug}?`,
    description: 'This deletes every ingested row and rollup but keeps the site and its settings. It cannot be undone.',
    action: 'Clear data',
    run: async () => {
      await api.clearSite(state.slug)
      return 'Cleared all rows and rollups'
    },
  }
}

const compact = () =>
  perform('Compact', async () => {
    const r = await api.compactSite(state.slug)
    return `Compacted: ${formatBytes(r.bytes_before)} to ${formatBytes(r.bytes_after)}`
  })

const reenrich = () =>
  perform('Re-check', async () => {
    const r = await api.reenrichSite(state.slug, state.dns)
    return `Bot requests: ${formatNumber(r.verified)} verified, ${formatNumber(r.fake)} fake, ${formatNumber(r.unverifiable)} unverifiable`
  })

watch(() => state.slug, load, { immediate: true })
</script>

<template>
  <div class="space-y-4">
    <div>
      <h1 class="text-xl font-semibold text-highlighted">Storage</h1>
      <p class="mt-0.5 text-sm text-muted">What is stored for {{ state.slug || 'the selected site' }}, and how to remove it.</p>
    </div>

    <UAlert v-if="!state.slug" color="warning" variant="subtle" title="No site selected" />
    <UAlert v-if="loadError" color="warning" variant="subtle" icon="i-lucide-info" :title="loadError" />

    <template v-if="currentSite?.stats">
      <dl class="grid grid-cols-2 gap-4 rounded-lg border border-default bg-default p-4 text-sm sm:grid-cols-4">
        <div>
          <dt class="text-xs text-muted">Size on disk</dt>
          <dd class="text-lg font-semibold tabular-nums text-highlighted">{{ formatBytes(currentSite.size_bytes) }}</dd>
        </div>
        <div>
          <dt class="text-xs text-muted">Requests</dt>
          <dd class="text-lg font-semibold tabular-nums text-highlighted">{{ formatNumber(currentSite.stats.rows) }}</dd>
        </div>
        <div>
          <dt class="text-xs text-muted">Covers</dt>
          <dd class="text-highlighted">{{ formatUtc(currentSite.stats.first_ts) }}<br />to {{ formatUtc(currentSite.stats.last_ts) }}</dd>
        </div>
        <div>
          <dt class="text-xs text-muted">Days kept only as rollups</dt>
          <dd class="text-lg font-semibold tabular-nums text-highlighted">{{ formatNumber(currentSite.stats.rollup_only_days) }}</dd>
        </div>
      </dl>

      <section class="rounded-lg border border-default bg-default">
        <header class="border-b border-default px-4 py-2.5">
          <h2 class="text-sm font-semibold text-highlighted">Ingested files</h2>
        </header>
        <div class="max-h-96 overflow-auto">
          <table class="w-full text-sm">
            <thead class="sticky top-0 bg-elevated text-left text-xs text-muted">
              <tr>
                <th class="px-4 py-2 font-medium">File</th>
                <th class="px-4 py-2 text-right font-medium">Rows</th>
                <th class="px-4 py-2 text-right font-medium">Bad</th>
                <th class="px-4 py-2 text-right font-medium">Skipped</th>
                <th class="px-4 py-2 font-medium">First request</th>
                <th class="px-4 py-2 font-medium">Last request</th>
                <th class="px-4 py-2 text-right font-medium">File size</th>
                <th class="px-4 py-2" />
              </tr>
            </thead>
            <tbody>
              <tr v-for="file in files" :key="file.file_id" class="border-t border-default">
                <td class="max-w-xs truncate px-4 py-1.5" :title="file.path">{{ fileName(file.path) }}</td>
                <td class="px-4 py-1.5 text-right tabular-nums">{{ formatNumber(file.rows_ok) }}</td>
                <td class="px-4 py-1.5 text-right tabular-nums">{{ formatNumber(file.rows_bad) }}</td>
                <td class="px-4 py-1.5 text-right tabular-nums">{{ formatNumber(file.rows_skipped) }}</td>
                <td class="whitespace-nowrap px-4 py-1.5">{{ formatUtc(file.first_ts) }}</td>
                <td class="whitespace-nowrap px-4 py-1.5">{{ formatUtc(file.last_ts) }}</td>
                <td class="px-4 py-1.5 text-right tabular-nums">{{ formatBytes(file.size_bytes) }}</td>
                <td class="px-4 py-1.5 text-right">
                  <UButton
                    icon="i-lucide-trash-2"
                    size="xs"
                    color="error"
                    variant="ghost"
                    aria-label="Delete this file's rows"
                    :disabled="!!working"
                    @click="askDeleteFile(file)"
                  />
                </td>
              </tr>
              <tr v-if="files.length === 0">
                <td colspan="8" class="px-4 py-6 text-center text-muted">No files ingested yet.</td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>

      <section class="grid gap-4 lg:grid-cols-2">
        <div class="rounded-lg border border-default bg-default p-4">
          <h2 class="text-sm font-semibold text-highlighted">Delete old rows</h2>
          <p class="mt-1 text-sm text-muted">Remove raw rows dated before a day (in the site's time zone).</p>
          <div class="mt-3 flex flex-wrap items-end gap-3">
            <UFormField label="Before">
              <UInput v-model="pruneBefore" type="date" />
            </UFormField>
            <UButton label="Delete rows" color="error" variant="outline" :disabled="!pruneBefore || !!working" @click="askPrune" />
          </div>
          <USwitch v-model="pruneRollups" class="mt-3" label="Delete the daily rollups for those days too" />
        </div>

        <div class="rounded-lg border border-default bg-default p-4">
          <h2 class="text-sm font-semibold text-highlighted">Maintenance</h2>
          <div class="mt-3 space-y-3 text-sm">
            <div class="flex items-center justify-between gap-4">
              <p class="text-muted">Reclaim disk space after deleting rows.</p>
              <UButton label="Compact" color="neutral" variant="outline" :loading="working === 'Compact'" :disabled="!!working" @click="compact" />
            </div>
            <div class="flex items-center justify-between gap-4">
              <p class="text-muted">Re-check bots against the current IP ranges and bot list.</p>
              <UButton label="Re-check bots" color="neutral" variant="outline" :loading="working === 'Re-check'" :disabled="!!working" @click="reenrich" />
            </div>
            <div class="flex items-center justify-between gap-4">
              <p class="text-muted">Delete all rows and rollups, keeping the site.</p>
              <UButton label="Clear data" color="error" variant="outline" :disabled="!!working" @click="askClear" />
            </div>
          </div>
        </div>
      </section>
    </template>

    <UModal
      :open="confirming !== null"
      :title="confirming?.title"
      :description="confirming?.description"
      @update:open="(open: boolean) => { if (!open) confirming = null }"
    >
      <template #footer>
        <div class="flex w-full justify-end gap-2">
          <UButton label="Cancel" color="neutral" variant="ghost" @click="confirming = null" />
          <UButton
            v-if="confirming"
            :label="confirming.action"
            color="error"
            :loading="!!working"
            @click="perform(confirming.action, confirming.run)"
          />
        </div>
      </template>
    </UModal>
  </div>
</template>
