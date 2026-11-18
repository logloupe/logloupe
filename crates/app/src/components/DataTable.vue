<script setup lang="ts">
// One report table. Only ever renders the page of rows it was given; paging
// asks the engine for the next page.
import { computed, ref, watch } from 'vue'
import { api, errorMessage, type Cell, type Column, type ReportTable } from '../api'
import { formatBytes, formatNumber, humanise } from '../format'
import { canFilterBy, clearFilters, filterBy, hasFilters, state } from '../state'

const props = defineProps<{
  table: ReportTable
  loading?: boolean
  /** The column the rows are ordered by, when it is not the table's usual order. */
  sort?: { key: string; descending: boolean } | null
}>()
const emit = defineEmits<{ page: [offset: number]; export: []; sort: [column: Column] }>()

function ariaSort(column: Column): 'ascending' | 'descending' | 'none' {
  if (props.sort?.key !== column.key) return 'none'
  return props.sort.descending ? 'descending' : 'ascending'
}

function sortHint(column: Column): string {
  const name = humanise(column.key).toLowerCase()
  const state = ariaSort(column)
  if (state === 'none') return `Sort by ${name}`
  return `Sorted by ${name}, ${state === 'descending' ? 'largest' : 'smallest'} first. Click to change.`
}

const toast = useToast()
const blockable = /^[0-9a-f:.]+(\/\d{1,3})?$/i

// Tables that list addresses or networks can be turned into a block list.
const targetIndex = computed(() => {
  const keys = props.table.columns.map((c) => c.key)
  for (const key of ['ip', 'network', 'target']) {
    const index = keys.indexOf(key)
    if (index >= 0) return index
  }
  return -1
})
const selectable = computed(() => targetIndex.value >= 0)
const selected = ref(new Set<string>())
const targets = computed(() =>
  props.table.rows
    .map((row) => row[targetIndex.value])
    // Only addresses and networks can be blocked; a provider or "(whole site)" cannot.
    .filter((v): v is string => typeof v === 'string' && blockable.test(v)),
)
const allSelected = computed(() => targets.value.length > 0 && targets.value.every((t) => selected.value.has(t)))

watch(
  () => props.table,
  () => {
    selected.value = new Set()
    preview.value = null
  },
)
// A different selection needs a new list; rebuild it in the style last chosen.
watch(selected, () => {
  if (!preview.value) return
  if (selected.value.size === 0) preview.value = null
  else void buildBlockList(preview.value.style)
})

function toggle(target: Cell) {
  if (typeof target !== 'string') return
  const next = new Set(selected.value)
  if (!next.delete(target)) next.add(target)
  selected.value = next
}

function toggleAll() {
  selected.value = allSelected.value ? new Set() : new Set(targets.value)
}

type BlockStyle = 'plain' | 'nginx' | 'apache'
const styleNames: Record<BlockStyle, string> = { plain: 'Plain list', nginx: 'nginx deny', apache: 'Apache Require' }
const preview = ref<{ style: BlockStyle; text: string; entries: number; leftOut: string[] } | null>(null)

/** Builds the list in the engine and shows it, so it can be read before use. */
async function buildBlockList(style: BlockStyle) {
  try {
    const list = await api.formatBlockList(state.slug, [...selected.value], style)
    preview.value = { style, text: list.text, entries: list.entries, leftOut: list.left_out }
  } catch (e) {
    preview.value = null
    toast.add({ title: 'Could not build the block list', description: errorMessage(e), color: 'error' })
  }
}

async function copyPreview() {
  if (!preview.value) return
  try {
    await navigator.clipboard.writeText(preview.value.text)
    toast.add({ title: `Copied ${preview.value.entries} ${preview.value.entries === 1 ? 'entry' : 'entries'}`, color: 'success' })
  } catch (e) {
    toast.add({ title: 'Could not copy. Select the text and copy it by hand.', description: errorMessage(e), color: 'error' })
  }
}

const numeric = (column: Column) => column.type === 'integer' || column.type === 'float'

function display(column: Column, value: Cell): string {
  if (value === null) return ''
  if (typeof value === 'number') {
    if (column.key.endsWith('bytes')) return formatBytes(value)
    // Identifiers are not quantities: no thousands separators.
    if (column.key === 'asn' || column.key === 'status' || column.key === 'last_status') return String(value)
    if (column.key === 'share') return `${value.toFixed(1)}%`
    return formatNumber(value)
  }
  return String(value)
}

const verdictColour: Record<string, string> = {
  verified: 'text-success',
  fake: 'text-error font-medium',
  unverifiable: 'text-muted',
}

function cellClass(column: Column, value: Cell): string {
  if (numeric(column)) return 'text-right tabular-nums'
  if ((column.key === 'verify' || column.key === 'bot_verify') && typeof value === 'string') {
    return verdictColour[value] ?? ''
  }
  return ''
}

const totals = computed(() =>
  Object.entries(props.table.totals).map(([key, value]) => {
    const column = { key, type: 'integer' } as Column
    return `${humanise(key)}: ${value === null ? 'n/a' : display(column, value)}`
  }),
)

const first = computed(() => (props.table.rows.length ? props.table.offset + 1 : 0))
const last = computed(() => props.table.offset + props.table.rows.length)
const hasPrevious = computed(() => props.table.offset > 0)
const hasNext = computed(() => last.value < props.table.total_rows)
</script>

<template>
  <section class="rounded-lg border border-default bg-default">
    <header class="flex items-center justify-between gap-3 border-b border-default px-4 py-2.5">
      <h3 class="text-sm font-semibold text-highlighted">{{ humanise(table.name) }}</h3>
      <div class="flex items-center gap-2 text-xs text-muted">
        <span v-if="table.total_rows > 0">
          {{ formatNumber(first) }}–{{ formatNumber(last) }} of {{ formatNumber(table.total_rows) }}
        </span>
        <UButton
          icon="i-lucide-chevron-left"
          size="xs"
          color="neutral"
          variant="ghost"
          aria-label="Previous page"
          :disabled="!hasPrevious || loading"
          @click="emit('page', Math.max(0, table.offset - table.limit))"
        />
        <UButton
          icon="i-lucide-chevron-right"
          size="xs"
          color="neutral"
          variant="ghost"
          aria-label="Next page"
          :disabled="!hasNext || loading"
          @click="emit('page', table.offset + table.limit)"
        />
        <UButton
          icon="i-lucide-download"
          size="xs"
          color="neutral"
          variant="outline"
          label="CSV"
          :disabled="table.total_rows === 0"
          @click="emit('export')"
        />
      </div>
    </header>

    <div
      v-if="selected.size > 0"
      class="flex flex-wrap items-center gap-2 border-b border-default bg-primary/8 px-4 py-2 text-xs"
    >
      <span class="font-medium text-highlighted">{{ selected.size }} selected. Make a block list:</span>
      <UButton
        v-for="(name, style) in styleNames"
        :key="style"
        :label="name"
        size="xs"
        :color="preview?.style === style ? 'primary' : 'neutral'"
        :variant="preview?.style === style ? 'solid' : 'outline'"
        @click="buildBlockList(style)"
      />
      <UButton label="Clear selection" size="xs" color="neutral" variant="link" @click="selected = new Set()" />
    </div>

    <div v-if="preview" class="border-b border-default bg-elevated/50 px-4 py-3">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <p class="text-xs text-muted">
          <span class="font-medium text-highlighted">{{ preview.entries }} {{ preview.entries === 1 ? 'entry' : 'entries' }}.</span>
          <template v-if="preview.leftOut.length">
            Left out {{ preview.leftOut.length }} verified bot or server
            {{ preview.leftOut.length === 1 ? 'address' : 'addresses' }}:
            {{ preview.leftOut.slice(0, 4).join(', ') }}{{ preview.leftOut.length > 4 ? '…' : '' }}.
          </template>
          Read it before using it: busy is not the same as hostile.
        </p>
        <UButton icon="i-lucide-copy" label="Copy" size="xs" :disabled="preview.entries === 0" @click="copyPreview" />
      </div>
      <pre
        v-if="preview.entries > 0"
        class="mt-2 max-h-48 overflow-auto rounded-md border border-default bg-default p-3 font-mono text-xs leading-5 text-highlighted select-all"
        >{{ preview.text }}</pre
      >
      <p v-else class="mt-2 text-xs text-muted">
        Everything selected is a verified bot or one of the server's own addresses, and those never go in a block list.
      </p>
    </div>

    <div class="max-h-[32rem] overflow-auto" :class="{ 'opacity-60': loading }">
      <table class="w-full border-collapse text-sm">
        <thead class="sticky top-0 z-10 bg-elevated text-left text-xs text-muted">
          <tr>
            <th v-if="selectable" class="w-8 bg-elevated py-2 pl-4">
              <input
                type="checkbox"
                class="size-3.5 cursor-pointer accent-(--ui-primary)"
                :checked="allSelected"
                aria-label="Select every row on this page"
                @change="toggleAll"
              />
            </th>
            <th
              v-for="column in table.columns"
              :key="column.key"
              class="whitespace-nowrap bg-elevated px-4 py-2 font-medium"
              :class="[
                numeric(column) ? 'text-right' : '',
                column.key === table.columns[0].key && !selectable ? 'sticky left-0 z-10' : '',
              ]"
              :aria-sort="ariaSort(column)"
            >
              <button
                type="button"
                class="group/sort inline-flex cursor-pointer items-center gap-1 rounded-sm font-medium hover:text-highlighted focus-visible:outline-2 focus-visible:outline-primary disabled:cursor-default"
                :class="[numeric(column) ? 'flex-row-reverse' : '', sort?.key === column.key ? 'text-highlighted' : '']"
                :title="sortHint(column)"
                :disabled="loading || table.total_rows < 2"
                @click="emit('sort', column)"
              >
                {{ humanise(column.key) }}
                <UIcon
                  :name="
                    sort?.key === column.key
                      ? sort.descending
                        ? 'i-lucide-arrow-down'
                        : 'i-lucide-arrow-up'
                      : 'i-lucide-arrow-up-down'
                  "
                  class="size-3 shrink-0"
                  :class="sort?.key === column.key ? 'text-primary' : 'opacity-0 group-hover/sort:opacity-60 group-focus-visible/sort:opacity-60'"
                />
              </button>
            </th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="(row, r) in table.rows" :key="r" class="group border-t border-default">
            <td v-if="selectable" class="w-8 py-1.5 pl-4 group-hover:bg-elevated">
              <input
                v-if="typeof row[targetIndex] === 'string' && blockable.test(String(row[targetIndex]))"
                type="checkbox"
                class="size-3.5 cursor-pointer accent-(--ui-primary)"
                :checked="typeof row[targetIndex] === 'string' && selected.has(String(row[targetIndex]))"
                :aria-label="`Select ${row[targetIndex]}`"
                @change="toggle(row[targetIndex])"
              />
            </td>
            <td
              v-for="(column, c) in table.columns"
              :key="column.key"
              class="max-w-[36rem] truncate px-4 py-1.5 group-hover:bg-elevated"
              :class="[cellClass(column, row[c]), c === 0 && !selectable ? 'sticky left-0 bg-default' : '']"
              :title="typeof row[c] === 'string' && !canFilterBy(column.key) ? String(row[c]) : undefined"
            >
              <button
                v-if="canFilterBy(column.key) && row[c] !== null && row[c] !== ''"
                type="button"
                class="max-w-full cursor-pointer truncate rounded-sm text-left decoration-primary/60 underline-offset-4 hover:text-primary hover:underline focus-visible:outline-2 focus-visible:outline-primary"
                :title="`Show only ${humanise(column.key).toLowerCase()} ${row[c]} in every report`"
                @click="filterBy(column.key, String(row[c]))"
              >
                {{ display(column, row[c]) }}
              </button>
              <template v-else>{{ display(column, row[c]) }}</template>
            </td>
          </tr>
          <tr v-if="table.rows.length === 0">
            <td :colspan="table.columns.length + (selectable ? 1 : 0)" class="px-4 py-8 text-center text-muted">
              <template v-if="hasFilters">
                <p class="text-highlighted">Nothing matches the current filters.</p>
                <UButton class="mt-2" label="Clear all filters" size="xs" color="neutral" variant="outline" @click="clearFilters" />
              </template>
              <template v-else>Nothing recorded for this period.</template>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <footer v-if="totals.length" class="border-t border-default px-4 py-2 text-xs text-muted">
      Totals: {{ totals.join(' · ') }}
    </footer>
  </section>
</template>
