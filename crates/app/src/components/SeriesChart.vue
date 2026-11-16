<script setup lang="ts">
// Time-series chart for the overview report.
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import uPlot from 'uplot'
import type { ReportTable } from '../api'
import { formatNumber } from '../format'

const props = defineProps<{ table: ReportTable; series: string[] }>()

const host = ref<HTMLDivElement>()
let plot: uPlot | undefined
let observer: ResizeObserver | undefined

// In the overview: requests, bot, human, pages. Red is kept for errors elsewhere.
const colours = ['#0d9488', '#7c3aed', '#d97706', '#0284c7', '#64748b']

/** Buckets are wall-clock times in the site's zone; plot them as if UTC. */
function seconds(bucket: string): number {
  const iso = bucket.length === 10 ? `${bucket}T00:00:00Z` : `${bucket.replace(' ', 'T')}Z`
  return Date.parse(iso) / 1000
}

function data(): uPlot.AlignedData {
  const keys = props.table.columns.map((c) => c.key)
  const x = props.table.rows.map((row) => seconds(String(row[0])))
  const ys = props.series.map((key) => {
    const index = keys.indexOf(key)
    return props.table.rows.map((row) => (index < 0 ? null : (row[index] as number | null)))
  })
  return [x, ...ys]
}

function draw() {
  plot?.destroy()
  plot = undefined
  if (!host.value || props.table.rows.length === 0) return
  const dark = document.documentElement.classList.contains('dark')
  const axis = dark ? '#94a3b8' : '#64748b'
  const grid = dark ? 'rgba(148,163,184,0.15)' : 'rgba(100,116,139,0.15)'
  plot = new uPlot(
    {
      width: host.value.clientWidth,
      height: 260,
      tzDate: (ts) => uPlot.tzDate(new Date(ts * 1000), 'UTC'),
      cursor: { drag: { x: false, y: false } },
      legend: { live: true },
      scales: { x: { time: true } },
      axes: [
        { stroke: axis, grid: { stroke: grid }, ticks: { stroke: grid } },
        {
          stroke: axis,
          grid: { stroke: grid },
          ticks: { stroke: grid },
          size: 70,
          values: (_u, ticks) => ticks.map((t) => formatNumber(t)),
        },
      ],
      series: [
        {},
        ...props.series.map((key, i) => ({
          label: key,
          stroke: colours[i % colours.length],
          width: 2,
          points: { show: props.table.rows.length < 60 },
          value: (_u: uPlot, v: number | null) => (v === null ? '-' : formatNumber(v)),
        })),
      ],
    },
    data(),
    host.value,
  )
  // Until the pointer is over the chart, the legend shows the latest bucket
  // rather than a row of dashes.
  plot.setLegend({ idx: props.table.rows.length - 1 })
}

onMounted(() => {
  draw()
  observer = new ResizeObserver(() => {
    if (plot && host.value) plot.setSize({ width: host.value.clientWidth, height: 260 })
  })
  if (host.value) observer.observe(host.value)
})

watch(() => [props.table, props.series], draw)

onBeforeUnmount(() => {
  observer?.disconnect()
  plot?.destroy()
})
</script>

<template>
  <div ref="host" class="w-full" />
</template>
