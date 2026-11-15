<script setup lang="ts">
// The date range and filters shared by every report, plus a strip that says
// what is currently applied. Filters stay on when switching reports, so the
// strip is the only thing that explains a narrowed or empty table.
import { computed, reactive, ref, watch } from 'vue'
import { activeFilters, clearFilters, refreshReports, removeFilter, state } from '../state'

const open = ref(false)

// Text boxes edit a draft. Nothing is filtered until it is applied, so the
// strip below never shows a filter that is not in force.
const draft = reactive({ bot: '', pathPrefix: '', ip: '', referrer: '', userAgent: '', country: '', asn: '' })
type DraftKey = keyof typeof draft
const draftKeys = Object.keys(draft) as DraftKey[]

watch(
  () => draftKeys.map((key) => state[key]),
  () => draftKeys.forEach((key) => (draft[key] = state[key])),
  { immediate: true },
)

function apply() {
  draftKeys.forEach((key) => (state[key] = draft[key].trim()))
  state.country = state.country.toUpperCase()
  state.asn = state.asn.replace(/^as/i, '').replace(/\D/g, '')
  refreshReports()
}

const statusItems = [
  { label: 'Any status', value: 'any' },
  { label: '2xx', value: '2' },
  { label: '3xx', value: '3' },
  { label: '4xx', value: '4' },
  { label: '5xx', value: '5' },
]
const sourceItems = [
  { label: 'Any source', value: 'any' },
  ...['direct', 'internal', 'search', 'ai', 'social', 'email', 'campaign', 'other'].map((value) => ({
    label: value === 'ai' ? 'AI' : value.charAt(0).toUpperCase() + value.slice(1),
    value,
  })),
]

// Choices apply as soon as they are made.
const statusModel = computed({
  get: () => state.statusClass || 'any',
  set: (value: string) => {
    state.statusClass = value === 'any' ? '' : value
    refreshReports()
  },
})
const sourceModel = computed({
  get: () => state.source || 'any',
  set: (value: string) => {
    state.source = value === 'any' ? '' : value
    refreshReports()
  },
})

watch(() => [state.from, state.to], refreshReports)

const hiddenCount = computed(() => activeFilters.value.filter((f) => f.key !== 'from' && f.key !== 'to').length)
</script>

<template>
  <header class="border-b border-default bg-default">
    <div class="flex flex-wrap items-end gap-3 px-6 py-3">
      <UFormField label="From" size="sm">
        <UInput v-model="state.from" type="date" size="sm" />
      </UFormField>
      <UFormField label="To" size="sm">
        <UInput v-model="state.to" type="date" size="sm" />
      </UFormField>
      <UButton
        icon="i-lucide-sliders-horizontal"
        :trailing-icon="open ? 'i-lucide-chevron-up' : 'i-lucide-chevron-down'"
        :label="hiddenCount && !open ? `Filters (${hiddenCount})` : 'Filters'"
        color="neutral"
        variant="outline"
        size="sm"
        :aria-expanded="open"
        aria-controls="report-filters"
        @click="open = !open"
      />
    </div>

    <form
      v-show="open"
      id="report-filters"
      class="flex flex-wrap items-end gap-3 border-t border-default px-6 py-3"
      @submit.prevent="apply"
    >
      <UFormField label="Status" size="sm">
        <USelect v-model="statusModel" :items="statusItems" size="sm" class="w-28" />
      </UFormField>
      <UFormField label="Source" size="sm">
        <USelect v-model="sourceModel" :items="sourceItems" size="sm" class="w-32" />
      </UFormField>
      <UFormField label="Path starts with" size="sm">
        <UInput v-model="draft.pathPrefix" placeholder="/blog/" size="sm" class="w-44" />
      </UFormField>
      <UFormField label="IP, /24 or /48" size="sm">
        <UInput v-model="draft.ip" placeholder="203.0.113.7" size="sm" class="w-40" />
      </UFormField>
      <UFormField label="Bot" size="sm">
        <UInput v-model="draft.bot" placeholder="googlebot" size="sm" class="w-32" />
      </UFormField>
      <UFormField label="Referrer contains" size="sm">
        <UInput v-model="draft.referrer" placeholder="(none)" size="sm" class="w-44" />
      </UFormField>
      <UFormField label="User agent contains" size="sm">
        <UInput v-model="draft.userAgent" placeholder="Chrome/131" size="sm" class="w-44" />
      </UFormField>
      <UFormField label="Country code" size="sm">
        <UInput v-model="draft.country" placeholder="GB" size="sm" class="w-24" />
      </UFormField>
      <UFormField label="Network (ASN)" size="sm">
        <UInput v-model="draft.asn" placeholder="15169" size="sm" class="w-28" />
      </UFormField>
      <UButton type="submit" label="Apply" size="sm" />
      <p class="basis-full text-xs text-muted">
        Type <code class="rounded bg-elevated px-1">(none)</code> to match requests with no referrer or no user agent.
        Filters apply to every report until you remove them.
      </p>
    </form>

    <div
      v-if="activeFilters.length"
      class="flex flex-wrap items-center gap-2 border-t border-default bg-primary/8 px-6 py-2"
      role="status"
      aria-label="Active filters"
    >
      <UIcon name="i-lucide-filter" class="size-4 shrink-0 text-primary" />
      <span class="text-xs font-medium text-highlighted">Showing only</span>
      <span
        v-for="filter in activeFilters"
        :key="filter.key"
        class="inline-flex max-w-md items-center gap-1 rounded-full border border-primary/30 bg-default py-0.5 pl-2.5 pr-1 text-xs text-highlighted"
      >
        <span class="truncate" :title="filter.label">{{ filter.label }}</span>
        <button
          type="button"
          class="grid size-5 shrink-0 cursor-pointer place-items-center rounded-full text-muted hover:bg-elevated hover:text-highlighted focus-visible:outline-2 focus-visible:outline-primary"
          :aria-label="`Remove filter: ${filter.label}`"
          @click="removeFilter(filter.key)"
        >
          <UIcon name="i-lucide-x" class="size-3.5" />
        </button>
      </span>
      <UButton label="Clear all" size="xs" color="neutral" variant="link" @click="clearFilters" />
    </div>
  </header>
</template>
