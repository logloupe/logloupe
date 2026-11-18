<script setup lang="ts">
// Sites: list with coverage and size, add, edit, delete.
import { onMounted, reactive, ref } from 'vue'
import { api, errorMessage, type SettingsPatch, type SiteStats } from '../api'
import { loadSites, selectSite, state } from '../state'
import { formatBytes, formatNumber, formatUtc } from '../format'

const emit = defineEmits<{ open: [view: string] }>()
const toast = useToast()

const retentionModes = [
  { label: 'Keep everything', value: 'forever' },
  { label: 'Keep raw rows for N days', value: 'days' },
  { label: 'Analyse then discard', value: 'none' },
]
const overlapModes = [
  { label: 'Ask (stop with an error)', value: 'error' },
  { label: 'Skip lines already covered', value: 'skip' },
  { label: 'Ingest everything', value: 'allow' },
]

/** The zone's offset from UTC today, or nothing when the webview does not know the zone. */
function utcOffset(zone: string, now: Date): string {
  try {
    const parts = new Intl.DateTimeFormat('en', { timeZone: zone, timeZoneName: 'longOffset' }).formatToParts(now)
    const offset = parts.find((p) => p.type === 'timeZoneName')?.value ?? ''
    return offset === 'GMT' ? 'UTC+00:00' : offset.replace('GMT', 'UTC')
  } catch {
    return ''
  }
}

// The engine's own list, so every choice is a name it accepts. Labels use
// spaces so that typing "new york" finds America/New_York.
const timezones = ref<{ label: string; value: string; offset: string }[]>([])
onMounted(async () => {
  try {
    const now = new Date()
    timezones.value = (await api.listTimezones()).map((value) => ({
      label: value.replaceAll('_', ' '),
      value,
      offset: utcOffset(value, now),
    }))
  } catch (e) {
    toast.add({ title: 'Could not load the time zone list', description: errorMessage(e), color: 'error' })
  }
})

/** This computer's zone, when the engine knows it by that name. */
function localTimezone(): string {
  const local = Intl.DateTimeFormat().resolvedOptions().timeZone
  return timezones.value.some((zone) => zone.value === local) ? local : 'UTC'
}

const blank = () => ({
  slug: '',
  hosts: '',
  timezone: localTimezone(),
  retentionMode: 'forever',
  retentionDays: 90,
  overlap: 'error',
  edgeCache: false,
})

const form = reactive(blank())
const formOpen = ref(false)
const editing = ref<string | null>(null)
const saving = ref(false)
const formError = ref('')
const removing = ref<string | null>(null)
const removeBusy = ref(false)

function openAdd() {
  Object.assign(form, blank())
  editing.value = null
  formError.value = ''
  formOpen.value = true
}

function openEdit(stats: SiteStats) {
  const days = stats.retention.startsWith('days:') ? Number(stats.retention.slice(5)) : 90
  Object.assign(form, {
    slug: stats.slug,
    hosts: stats.hosts.join(', '),
    timezone: stats.timezone,
    retentionMode: stats.retention.startsWith('days:') ? 'days' : stats.retention,
    retentionDays: days,
    overlap: stats.overlap,
    edgeCache: stats.edge_cache,
  })
  editing.value = stats.slug
  formError.value = ''
  formOpen.value = true
}

function patch(): SettingsPatch {
  return {
    hosts: form.hosts.split(/[\s,]+/).filter(Boolean),
    timezone: form.timezone,
    retention: form.retentionMode === 'days' ? `days:${form.retentionDays}` : form.retentionMode,
    overlap: form.overlap,
    edge_cache: form.edgeCache,
  }
}

async function submit() {
  saving.value = true
  formError.value = ''
  try {
    if (editing.value) {
      const change = await api.updateSite(editing.value, patch())
      if (change.hosts_changed) {
        toast.add({
          title: 'Hostnames changed',
          description: 'Rows already ingested keep their old source types. Re-ingest with "Replace" to apply the new hostnames.',
          color: 'warning',
        })
      }
      if (change.stale_rollup_days > 0) {
        toast.add({
          title: 'Time zone changed',
          description: `${change.stale_rollup_days} day(s) exist only as rollups and keep their old day boundaries.`,
          color: 'warning',
        })
      }
    } else {
      const slug = form.slug.trim()
      await api.addSite(slug, patch())
      selectSite(slug)
    }
    await loadSites()
    formOpen.value = false
  } catch (e) {
    formError.value = errorMessage(e)
  } finally {
    saving.value = false
  }
}

async function confirmRemove() {
  if (!removing.value) return
  removeBusy.value = true
  try {
    await api.removeSite(removing.value)
    toast.add({ title: `Deleted ${removing.value}`, color: 'success' })
    removing.value = null
    await loadSites()
  } catch (e) {
    toast.add({ title: 'Could not delete the site', description: errorMessage(e), color: 'error' })
  } finally {
    removeBusy.value = false
  }
}

function open(slug: string, view: string) {
  selectSite(slug)
  emit('open', view)
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <div>
        <h1 class="text-xl font-semibold text-highlighted">Sites</h1>
        <p class="mt-0.5 text-sm text-muted">Each site has its own database file. Deleting a site frees its disk space at once.</p>
      </div>
      <UButton icon="i-lucide-plus" label="Add site" @click="openAdd" />
    </div>

    <div
      v-if="state.sitesLoaded && state.sites.length === 0"
      class="rounded-lg border border-dashed border-default px-6 py-12 text-center"
    >
      <p class="font-medium text-highlighted">No sites yet</p>
      <p class="mt-1 text-sm text-muted">Add a site, then drop its access logs onto the Ingest screen.</p>
      <UButton class="mt-4" icon="i-lucide-plus" label="Add your first site" @click="openAdd" />
    </div>

    <div v-for="site in state.sites" :key="site.slug" class="rounded-lg border border-default bg-default p-4">
      <div class="flex items-start justify-between gap-4">
        <div class="min-w-0">
          <div class="flex items-center gap-2">
            <h2 class="truncate text-base font-semibold text-highlighted">{{ site.slug }}</h2>
            <UBadge v-if="site.slug === state.slug" label="Selected" color="primary" variant="subtle" size="sm" />
            <UBadge v-if="site.locked" label="In use" color="warning" variant="subtle" size="sm" />
            <UBadge v-if="site.stats?.edge_cache" label="Behind CDN cache" color="neutral" variant="subtle" size="sm" />
          </div>
          <p v-if="site.stats" class="mt-0.5 truncate text-sm text-muted">{{ site.stats.hosts.join(', ') }}</p>
          <p v-else class="mt-0.5 text-sm text-muted">Another process has this site open, so only its size is known.</p>
        </div>
        <div class="flex shrink-0 gap-2">
          <UButton label="Reports" size="sm" color="neutral" variant="outline" :disabled="site.locked" @click="open(site.slug, 'overview')" />
          <UButton label="Ingest" size="sm" color="neutral" variant="outline" :disabled="site.locked" @click="open(site.slug, 'ingest')" />
          <UButton
            v-if="site.stats"
            icon="i-lucide-pencil"
            size="sm"
            color="neutral"
            variant="ghost"
            aria-label="Edit site"
            @click="openEdit(site.stats)"
          />
          <UButton icon="i-lucide-trash-2" size="sm" color="error" variant="ghost" aria-label="Delete site" @click="removing = site.slug" />
        </div>
      </div>

      <dl v-if="site.stats" class="mt-4 grid grid-cols-2 gap-x-6 gap-y-3 text-sm sm:grid-cols-3 lg:grid-cols-6">
        <div>
          <dt class="text-xs text-muted">Requests</dt>
          <dd class="tabular-nums text-highlighted">{{ formatNumber(site.stats.rows) }}</dd>
        </div>
        <div>
          <dt class="text-xs text-muted">First request</dt>
          <dd class="text-highlighted">{{ formatUtc(site.stats.first_ts) }}</dd>
        </div>
        <div>
          <dt class="text-xs text-muted">Last request</dt>
          <dd class="text-highlighted">{{ formatUtc(site.stats.last_ts) }}</dd>
        </div>
        <div>
          <dt class="text-xs text-muted">Size on disk</dt>
          <dd class="tabular-nums text-highlighted">{{ formatBytes(site.size_bytes) }}</dd>
        </div>
        <div>
          <dt class="text-xs text-muted">Retention</dt>
          <dd class="text-highlighted">{{ site.stats.retention }}</dd>
        </div>
        <div>
          <dt class="text-xs text-muted">Time zone</dt>
          <dd class="text-highlighted">{{ site.stats.timezone }}</dd>
        </div>
      </dl>
      <p v-else class="mt-3 text-sm tabular-nums text-muted">{{ formatBytes(site.size_bytes) }} on disk</p>
    </div>

    <UModal v-model:open="formOpen" :title="editing ? `Edit ${editing}` : 'Add site'" :description="editing ? 'Change this site\'s settings.' : 'Create a site to ingest logs into.'">
      <template #body>
        <form id="site-form" class="space-y-4" @submit.prevent="submit">
          <UFormField v-if="!editing" label="Name" help="Lowercase letters, digits, dots and dashes. Used as the file name.">
            <UInput v-model="form.slug" placeholder="example.com" class="w-full" required autofocus />
          </UFormField>
          <UFormField label="Hostnames" help="The site's own hostnames, separated by commas. Referrers from these count as internal.">
            <UInput v-model="form.hosts" placeholder="example.com, www.example.com" class="w-full" required />
          </UFormField>
          <UFormField label="Time zone" help="Decides what a day and an hour mean in reports. Type a city or region to search.">
            <USelectMenu
              v-model="form.timezone"
              :items="timezones"
              value-key="value"
              :filter-fields="['label', 'value', 'offset']"
              :search-input="{ placeholder: 'Search, for example London or UTC+01' }"
              virtualize
              class="w-full"
            >
              <template #item-trailing="{ item }">
                <span class="text-xs tabular-nums text-muted">{{ item.offset }}</span>
              </template>
            </USelectMenu>
          </UFormField>
          <div class="grid grid-cols-2 gap-4">
            <UFormField label="Retention">
              <USelect v-model="form.retentionMode" :items="retentionModes" class="w-full" />
            </UFormField>
            <UFormField v-if="form.retentionMode === 'days'" label="Days to keep">
              <UInput v-model.number="form.retentionDays" type="number" min="1" class="w-full" />
            </UFormField>
          </div>
          <UFormField label="Files that overlap ones already ingested">
            <USelect v-model="form.overlap" :items="overlapModes" class="w-full" />
          </UFormField>
          <USwitch
            v-model="form.edgeCache"
            label="Behind a caching CDN"
            description="The log only holds requests that reached the origin server. Reports will say so."
          />
          <UAlert v-if="formError" color="error" variant="subtle" :title="formError" />
        </form>
      </template>
      <template #footer>
        <div class="flex w-full justify-end gap-2">
          <UButton label="Cancel" color="neutral" variant="ghost" @click="formOpen = false" />
          <UButton type="submit" form="site-form" :label="editing ? 'Save' : 'Add site'" :loading="saving" />
        </div>
      </template>
    </UModal>

    <UModal
      :open="removing !== null"
      :title="`Delete ${removing}?`"
      description="This deletes the site's database, including all ingested rows and rollups. It cannot be undone. The original log files are not touched."
      @update:open="(open: boolean) => { if (!open) removing = null }"
    >
      <template #footer>
        <div class="flex w-full justify-end gap-2">
          <UButton label="Cancel" color="neutral" variant="ghost" @click="removing = null" />
          <UButton label="Delete site" color="error" :loading="removeBusy" @click="confirmRemove" />
        </div>
      </template>
    </UModal>
  </div>
</template>
