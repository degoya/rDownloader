<script setup lang="ts">
/**
 * Bytes per bucket of the statistics view (RD-110-01), as bars.
 *
 * Bars rather than the throughput line: a bucket is a sum over an hour or a day, not a
 * sample of a rate, and a line would invite reading a slope between two sums that means
 * nothing. Same frame, grid and accessible label as `SpeedHistoryChart.vue`.
 *
 * The bars sit on the range's time axis, one slot per hour or day from `since` to now. The
 * response lists only the buckets that moved something, and laying those out by index drew a
 * single busy hour as one block across the whole width, with the same moment at both ends of
 * the axis, and closed every gap between two busy days (RD-120-48).
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { TransferStatsBucket } from '@/api/types'
import { formatBytes, formatDay, formatMoment } from '@/utils/format'

const { t } = useI18n()

const props = defineProps<{
  buckets: TransferStatsBucket[]
  resolution: 'hour' | 'day'
  /** Start of the range, as the response names it. */
  since: string
}>()

const WIDTH = 640
const HEIGHT = 100
const TOP = 8
const BOTTOM = 96

const peak = computed(() => props.buckets.reduce((max, bucket) => Math.max(max, Number(bucket.bytes)), 0))

/** The time axis: the first slot is the one `since` falls into, the last the current one. */
const axis = computed(() => {
  const width = props.resolution === 'hour' ? 3_600_000 : 86_400_000
  const origin = Math.floor(Date.parse(props.since) / width) * width
  const slotOf = (moment: number) => Math.max(0, Math.floor((moment - origin) / width))
  const newest = props.buckets.reduce((max, bucket) => Math.max(max, slotOf(Date.parse(bucket.start))), 0)
  const slots = Math.max(slotOf(Date.now()), newest) + 1
  return { width, origin, slots, slotOf }
})

const bars = computed(() => {
  if (!props.buckets.length) return []
  const { slots, slotOf } = axis.value
  const slot = WIDTH / slots
  const gap = Math.min(4, slot * 0.2)
  const scale = Math.max(1, peak.value)
  return props.buckets.map((bucket) => {
    const height = Number(bucket.bytes) / scale * (BOTTOM - TOP)
    return {
      key: bucket.start,
      x: slotOf(Date.parse(bucket.start)) * slot + gap / 2,
      width: Math.max(1, slot - gap),
      y: BOTTOM - height,
      height,
      title: `${formatLabel(bucket.start)} · ${formatBytes(String(bucket.bytes))}`
    }
  })
})

function formatLabel(start: string): string {
  return props.resolution === 'hour' ? formatMoment(start) : formatDay(start)
}

const first = computed(() => new Date(axis.value.origin).toISOString())
const last = computed(() => new Date(axis.value.origin + (axis.value.slots - 1) * axis.value.width).toISOString())
const accessibleLabel = computed(() => t('stats.chart.description', { peak: formatBytes(String(peak.value)) }))
</script>

<template>
  <figure class="border border-muted bg-default p-4">
    <figcaption class="mb-3 flex flex-wrap items-end justify-between gap-3">
      <div>
        <p class="eyebrow">{{ t('stats.chart.title') }}</p>
        <p class="mt-1 text-sm text-muted">{{ props.resolution === 'hour' ? t('stats.chart.subtitle_hour') : t('stats.chart.subtitle_day') }}</p>
      </div>
      <span class="numeric text-lg font-semibold text-highlighted">{{ formatBytes(String(peak)) }}</span>
    </figcaption>
    <p v-if="!bars.length" class="py-6 text-center text-sm text-muted">{{ t('stats.chart.empty') }}</p>
    <svg
      v-else
      :viewBox="`0 0 ${WIDTH} ${HEIGHT}`"
      class="h-28 w-full overflow-visible"
      role="img"
      :aria-label="accessibleLabel"
      preserveAspectRatio="none"
    >
      <g class="text-muted/50" stroke="currentColor" stroke-width="0.6" stroke-dasharray="3 5" vector-effect="non-scaling-stroke">
        <line v-for="ratio in [0, 0.5, 1]" :key="ratio" x1="0" :y1="TOP + (BOTTOM - TOP) * ratio" :x2="WIDTH" :y2="TOP + (BOTTOM - TOP) * ratio" />
      </g>
      <rect
        v-for="bar in bars"
        :key="bar.key"
        :x="bar.x"
        :y="bar.y"
        :width="bar.width"
        :height="bar.height"
        fill="var(--ui-primary)"
        fill-opacity="0.8"
      >
        <title>{{ bar.title }}</title>
      </rect>
      <line x1="0" :y1="BOTTOM" :x2="WIDTH" :y2="BOTTOM" stroke="currentColor" class="text-muted" stroke-width="0.8" vector-effect="non-scaling-stroke" />
    </svg>
    <div v-if="bars.length" class="numeric mt-1 flex justify-between text-[10px] text-muted" aria-hidden="true">
      <span>{{ first ? formatLabel(first) : '' }}</span>
      <span>{{ last ? formatLabel(last) : '' }}</span>
    </div>
  </figure>
</template>
