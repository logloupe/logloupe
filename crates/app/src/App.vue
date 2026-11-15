<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { errorMessage, inTauri } from './api'
import { currentSite, loadSites, selectSite, state } from './state'
import FilterBar from './components/FilterBar.vue'
import IngestView from './views/IngestView.vue'
import ReportView from './views/ReportView.vue'
import SettingsView from './views/SettingsView.vue'
import SitesView from './views/SitesView.vue'
import StorageView from './views/StorageView.vue'

interface NavItem {
  id: string
  label: string
  icon: string
  /** Set for report screens. */
  description?: string
}

const groups: { title: string; items: NavItem[] }[] = [
  {
    title: 'Traffic',
    items: [
      { id: 'overview', label: 'Overview', icon: 'i-lucide-chart-line', description: 'Requests, pages, bytes, unique IPs, the human and bot split, and status classes.' },
      { id: 'sources', label: 'Sources', icon: 'i-lucide-signpost', description: 'Where page views came from: source type, referrer host and full referrer.' },
      { id: 'campaigns', label: 'Campaigns', icon: 'i-lucide-megaphone', description: 'Page views by UTM tags and by ad click IDs.' },
      { id: 'landing-pages', label: 'Landing pages', icon: 'i-lucide-door-open', description: 'The first page of each approximate session.' },
      { id: 'geography', label: 'Geography', icon: 'i-lucide-earth', description: 'Pages and requests by country and by network (ASN).' },
      { id: 'ai-referrals', label: 'AI referrals', icon: 'i-lucide-sparkles', description: 'Visits sent by AI assistants, by assistant and landing page.' },
    ],
  },
  {
    title: 'Bots',
    items: [
      { id: 'bots', label: 'All bots', icon: 'i-lucide-bot', description: 'Every registered bot, with how much of its traffic is verified against published IP ranges.' },
      { id: 'fake-bots', label: 'Fake bots', icon: 'i-lucide-shield-alert', description: 'Requests using a known bot\'s user agent from an address outside that bot\'s published ranges.' },
      { id: 'bot-ips', label: 'Bot IPs', icon: 'i-lucide-network', description: 'Activity per IP address for registered bots. Use the Bot filter to look at one.' },
      { id: 'ai-crawlers', label: 'AI crawlers', icon: 'i-lucide-brain', description: 'AI training crawlers, AI search crawlers and user-triggered AI fetches.' },
      { id: 'undeclared', label: 'Undeclared', icon: 'i-lucide-ghost', description: 'Tools, HTTP libraries, and empty or unrecognised user agents.' },
    ],
  },
  {
    title: 'Abuse',
    items: [
      { id: 'status-codes', label: 'Status codes', icon: 'i-lucide-activity', description: 'Response codes over time, per code, and the URLs and addresses behind the errors.' },
      { id: 'top-ips', label: 'Top IPs', icon: 'i-lucide-flame', description: 'The busiest addresses, networks, user agents and paths, with peak requests per minute.' },
      { id: 'referrers', label: 'Referrers', icon: 'i-lucide-link', description: 'Referrers exactly as sent, including the site\'s own pages, with who sent them.' },
    ],
  },
  {
    title: 'Manage',
    items: [
      { id: 'ingest', label: 'Ingest', icon: 'i-lucide-file-down' },
      { id: 'storage', label: 'Storage', icon: 'i-lucide-database' },
      { id: 'sites', label: 'Sites', icon: 'i-lucide-globe' },
      { id: 'settings', label: 'Settings', icon: 'i-lucide-settings' },
    ],
  },
]

const view = ref('sites')
const startError = ref('')

const active = computed(() => groups.flatMap((g) => g.items).find((i) => i.id === view.value))
const isReport = computed(() => !!active.value?.description)
const siteItems = computed(() => state.sites.map((s) => ({ label: s.slug, value: s.slug })))

onMounted(async () => {
  try {
    await loadSites()
    if (currentSite.value?.stats?.rows) view.value = 'overview'
    else if (state.sites.length) view.value = 'ingest'
  } catch (e) {
    startError.value = errorMessage(e)
  }
})
</script>

<template>
  <UApp>
    <div class="flex h-full bg-muted text-default">
      <aside class="flex w-56 shrink-0 flex-col border-r border-default bg-default">
        <div class="border-b border-default p-3">
          <p class="px-1 text-xs font-medium text-muted">Site</p>
          <USelect
            v-if="siteItems.length"
            :model-value="state.slug"
            :items="siteItems"
            class="mt-1 w-full"
            @update:model-value="(slug: string) => selectSite(slug)"
          />
          <UButton v-else class="mt-1 w-full" label="Add a site" icon="i-lucide-plus" variant="outline" color="neutral" @click="view = 'sites'" />
        </div>
        <nav class="flex-1 overflow-y-auto p-2">
          <div v-for="group in groups" :key="group.title" class="mb-3">
            <p class="px-2 pb-1 text-xs font-medium text-muted">{{ group.title }}</p>
            <button
              v-for="item in group.items"
              :key="item.id"
              type="button"
              class="flex w-full cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm transition-colors focus-visible:outline-2 focus-visible:outline-primary"
              :class="view === item.id ? 'bg-primary/10 font-medium text-primary' : 'text-default hover:bg-elevated'"
              :aria-current="view === item.id ? 'page' : undefined"
              @click="view = item.id"
            >
              <UIcon :name="item.icon" class="size-4 shrink-0" />
              {{ item.label }}
            </button>
          </div>
        </nav>
        <p v-if="state.busy" class="border-t border-default px-3 py-2 text-xs text-muted">Working on {{ state.slug }}…</p>
      </aside>

      <div class="flex min-w-0 flex-1 flex-col">
        <FilterBar v-if="isReport" />

        <main class="flex-1 overflow-y-auto px-6 py-5">
          <UAlert
            v-if="!inTauri"
            class="mb-4"
            color="info"
            variant="subtle"
            icon="i-lucide-info"
            title="Browser preview with sample data"
            description="Open the desktop app to work with real logs."
          />
          <UAlert v-if="startError" class="mb-4" color="error" variant="subtle" title="Could not open the data folder" :description="startError" />

          <template v-if="isReport && active">
            <UAlert
              v-if="!state.slug"
              color="warning"
              variant="subtle"
              title="No site selected"
              description="Add a site and ingest some logs to see reports."
            />
            <ReportView v-else :key="active.id" :report="active.id" :title="active.label" :description="active.description ?? ''" />
          </template>
          <IngestView v-else-if="view === 'ingest'" />
          <StorageView v-else-if="view === 'storage'" />
          <SettingsView v-else-if="view === 'settings'" />
          <SitesView v-else @open="(target: string) => (view = target)" />
        </main>
      </div>
    </div>
  </UApp>
</template>
