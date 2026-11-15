<script setup lang="ts">
// One report table. Only ever renders the page of rows it was given; paging
// asks the engine for the next page.
import { computed } from 'vue'
import type { Cell, Column, ReportTable } from '../api'
import { formatBytes, formatNumber, humanise } from '../format'
import { canFilterBy, clearFilters, filterBy, hasFilters } from '../state'

const props = defineProps<{ table: ReportTable; loading?: boolean }>()
const emit = defineEmits<{ page: [offset: number]; export: [] }>()

const numeric = (column: Column) => column.type === 'integer' || column.type === 'float'

function display(column: Column, value: Cell): string {
  if (value === null) return ''
  if (typeof value === 'number') {
    if (column.key === 'bytes') return formatBytes(value)
    // Identifiers are not quantities: no thousands separators.
    if (column.key === 'asn') return String(value)
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

    <div class="max-h-[32rem] overflow-auto" :class="{ 'opacity-60': loading }">
      <table class="w-full border-collapse text-sm">
        <thead class="sticky top-0 z-10 bg-elevated text-left text-xs text-muted">
          <tr>
            <th
              v-for="column in table.columns"
              :key="column.key"
              class="whitespace-nowrap bg-elevated px-4 py-2 font-medium"
              :class="[numeric(column) ? 'text-right' : '', column.key === table.columns[0].key ? 'sticky left-0 z-10' : '']"
            >
              {{ humanise(column.key) }}
            </th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="(row, r) in table.rows" :key="r" class="group border-t border-default">
            <td
              v-for="(column, c) in table.columns"
              :key="column.key"
              class="max-w-[36rem] truncate px-4 py-1.5 group-hover:bg-elevated"
              :class="[cellClass(column, row[c]), c === 0 ? 'sticky left-0 bg-default' : '']"
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
            <td :colspan="table.columns.length" class="px-4 py-8 text-center text-muted">
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
