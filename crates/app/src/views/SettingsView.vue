<script setup lang="ts">
// Settings: the cached bot IP ranges, and where the data lives.
import { onMounted, ref } from 'vue'
import { api, errorMessage, type GeoStatus, type RangeStatus } from '../api'
import { formatBytes, formatNumber, formatUtc } from '../format'
import { loadSites, refreshReports, state } from '../state'

const toast = useToast()

const ranges = ref<RangeStatus[]>([])
const info = ref<{ version: string; data_dir: string } | null>(null)
const updating = ref(false)
const geo = ref<GeoStatus | null>(null)
const geoUpdating = ref(false)
const rechecking = ref(false)
const problems = ref<string[]>([])

async function load() {
  try {
    ;[ranges.value, info.value, geo.value] = await Promise.all([api.rangesStatus(), api.appInfo(), api.geoStatus()])
  } catch (e) {
    toast.add({ title: 'Could not load settings', description: errorMessage(e), color: 'error' })
  }
}

async function update() {
  updating.value = true
  problems.value = []
  try {
    const outcomes = await api.rangesUpdate()
    problems.value = outcomes
      .filter((o) => !o.updated)
      .map((o) => `${o.operator}: kept the previous copy (${o.message ?? 'unknown reason'})`)
    const updated = outcomes.filter((o) => o.updated).length
    toast.add({ title: `Updated ${updated} of ${outcomes.length} operators`, color: problems.value.length ? 'warning' : 'success' })
    await load()
    await recheck()
  } catch (e) {
    toast.add({ title: 'Update failed', description: errorMessage(e), color: 'error' })
  } finally {
    updating.value = false
  }
}

/** New ranges mean nothing until stored rows are checked against them. */
async function recheck() {
  if (!state.slug) return
  rechecking.value = true
  state.busy = true
  try {
    const r = await api.reenrichSite(state.slug)
    toast.add({
      title: `Re-checked bots in ${state.slug}`,
      description: `${formatNumber(r.verified)} verified, ${formatNumber(r.fake)} fake, ${formatNumber(r.unverifiable)} unverifiable.`,
      color: 'success',
    })
  } catch (e) {
    toast.add({
      title: `Could not re-check ${state.slug}`,
      description: `${errorMessage(e)} Use "Re-check bots" on the Storage screen to try again.`,
      color: 'error',
    })
  } finally {
    rechecking.value = false
    state.busy = false
    await loadSites()
    refreshReports()
  }
}

async function updateGeo() {
  geoUpdating.value = true
  try {
    geo.value = await api.geoUpdate()
    toast.add({ title: 'Geo databases downloaded', color: 'success' })
    await recheck()
  } catch (e) {
    toast.add({ title: 'Download failed', description: errorMessage(e), color: 'error' })
  } finally {
    geoUpdating.value = false
  }
}

onMounted(load)
</script>

<template>
  <div class="space-y-4">
    <div>
      <h1 class="text-xl font-semibold text-highlighted">Settings</h1>
      <p class="mt-0.5 text-sm text-muted">Nothing is sent anywhere. The only requests the app makes are to fetch the IP lists below.</p>
    </div>

    <section class="rounded-lg border border-default bg-default">
      <header class="flex items-center justify-between gap-4 border-b border-default px-4 py-2.5">
        <div>
          <h2 class="text-sm font-semibold text-highlighted">Bot IP ranges</h2>
          <p class="text-xs text-muted">
            Published by each operator. Used to tell real bots from fakes. Refreshed before an ingest when older than a
            day. Updating here also re-checks the selected site's stored requests; other sites have "Re-check bots" on
            their Storage screen.
          </p>
        </div>
        <UButton
          icon="i-lucide-refresh-cw"
          :label="rechecking ? 'Re-checking bots…' : 'Update now'"
          size="sm"
          :loading="updating || rechecking"
          @click="update"
        />
      </header>
      <table class="w-full text-sm">
        <thead class="bg-elevated text-left text-xs text-muted">
          <tr>
            <th class="px-4 py-2 font-medium">Operator</th>
            <th class="px-4 py-2 text-right font-medium">Prefixes</th>
            <th class="px-4 py-2 text-right font-medium">Retired</th>
            <th class="px-4 py-2 font-medium">Tracked since</th>
            <th class="px-4 py-2 font-medium">Last fetched</th>
            <th class="px-4 py-2 text-right font-medium">Fetches</th>
            <th class="px-4 py-2 font-medium">State</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="row in ranges" :key="row.operator" class="border-t border-default">
            <td class="px-4 py-1.5 text-highlighted">{{ row.name }}</td>
            <td class="px-4 py-1.5 text-right tabular-nums">{{ formatNumber(row.prefixes) }}</td>
            <td class="px-4 py-1.5 text-right tabular-nums">{{ formatNumber(row.retired) }}</td>
            <td class="px-4 py-1.5">{{ row.tracked_since ?? '-' }}</td>
            <td class="px-4 py-1.5">{{ row.fetched_at ? formatUtc(row.fetched_at) : 'Never' }}</td>
            <td class="px-4 py-1.5 text-right tabular-nums">{{ formatNumber(row.fetches) }}</td>
            <td class="px-4 py-1.5">
              <UBadge :label="row.stale ? 'Stale' : 'Fresh'" :color="row.stale ? 'warning' : 'success'" variant="subtle" size="sm" />
            </td>
          </tr>
        </tbody>
      </table>
      <footer class="border-t border-default px-4 py-2 text-xs text-muted">
        Retired prefixes are kept with the date they left the published list, so older logs are judged against what was
        published at the time.
      </footer>
    </section>

    <section v-if="geo" class="rounded-lg border border-default bg-default">
      <header class="flex items-center justify-between gap-4 border-b border-default px-4 py-2.5">
        <div>
          <h2 class="text-sm font-semibold text-highlighted">Country and network lookup</h2>
          <p class="text-xs text-muted">
            Adds a country and a network (ASN) to every address. "Download" fetches the free DB-IP Lite databases
            (about 10 MB), then fills them in for the selected site's stored requests.
          </p>
        </div>
        <UButton
          icon="i-lucide-download"
          :label="rechecking && geoUpdating ? 'Applying…' : geo.files.some((f) => f.present) ? 'Update' : 'Download'"
          size="sm"
          :loading="geoUpdating"
          @click="updateGeo"
        />
      </header>
      <table class="w-full text-sm">
        <thead class="bg-elevated text-left text-xs text-muted">
          <tr>
            <th class="px-4 py-2 font-medium">Database</th>
            <th class="px-4 py-2 font-medium">Provider</th>
            <th class="px-4 py-2 font-medium">Built</th>
            <th class="px-4 py-2 text-right font-medium">Size</th>
            <th class="px-4 py-2 font-medium">State</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="file in geo.files" :key="file.kind" class="border-t border-default">
            <td class="px-4 py-1.5 text-highlighted">{{ file.kind === 'asn' ? 'Network (ASN)' : 'Country' }}</td>
            <td class="px-4 py-1.5">{{ file.provider ?? 'Not installed' }}</td>
            <td class="px-4 py-1.5">{{ file.built_at ? formatUtc(file.built_at) : '-' }}</td>
            <td class="px-4 py-1.5 text-right tabular-nums">{{ file.size_bytes ? formatBytes(file.size_bytes) : '-' }}</td>
            <td class="px-4 py-1.5">
              <UBadge
                :label="!file.present ? 'Missing' : file.stale ? 'Stale' : 'Fresh'"
                :color="!file.present ? 'neutral' : file.stale ? 'warning' : 'success'"
                variant="subtle"
                size="sm"
              />
            </td>
          </tr>
        </tbody>
      </table>
      <footer class="border-t border-default px-4 py-2 text-xs text-muted">
        <template v-if="geo.attribution">
          IP geolocation by <a class="underline" href="https://db-ip.com" target="_blank" rel="noreferrer">DB-IP</a>.
        </template>
        To use MaxMind instead, put your own <code>GeoLite2-Country.mmdb</code> and <code>GeoLite2-ASN.mmdb</code> in
        the <code>geo</code> folder inside the data folder below; they take priority.
      </footer>
    </section>

    <UAlert v-for="problem in problems" :key="problem" color="warning" variant="subtle" icon="i-lucide-info" :title="problem" />

    <section class="flex items-center justify-between gap-4 rounded-lg border border-default bg-default p-4">
      <div>
        <h2 class="text-sm font-semibold text-highlighted">Appearance</h2>
        <p class="text-xs text-muted">Light, dark, or follow the system setting.</p>
      </div>
      <UColorModeSelect size="sm" class="w-36" />
    </section>

    <section v-if="info" class="rounded-lg border border-default bg-default p-4 text-sm">
      <h2 class="text-sm font-semibold text-highlighted">About</h2>
      <dl class="mt-2 space-y-1">
        <div class="flex gap-2">
          <dt class="w-28 shrink-0 text-muted">Version</dt>
          <dd class="text-highlighted">{{ info.version }}</dd>
        </div>
        <div class="flex gap-2">
          <dt class="w-28 shrink-0 text-muted">Data folder</dt>
          <dd class="break-all font-mono text-xs text-highlighted">{{ info.data_dir }}</dd>
        </div>
      </dl>
    </section>
  </div>
</template>
