<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { formatRate } from '@/utils/format'
import type { TransferRateHistoryPoint } from '@/utils/transferRates'

const { t } = useI18n()

const props = defineProps<{
  currentRate: number
  points: TransferRateHistoryPoint[]
  compact?: boolean
}>()

const WIDTH = 640
const HEIGHT = 100
const TOP = 8
const BOTTOM = 96
const WINDOW_MS = 2 * 60 * 1_000

const peakRate = computed(() => Math.max(props.currentRate, ...props.points.map(point => point.bytesPerSecond), 0))
const scaleMaximum = computed(() => Math.max(1, peakRate.value * 1.08))
const coordinates = computed(() => {
  if (!props.points.length) return []
  const last = props.points.at(-1)?.measuredAt ?? 0
  const first = last - WINDOW_MS
  return props.points.map(point => ({
    x: Math.max(0, Math.min(WIDTH, (point.measuredAt - first) / WINDOW_MS * WIDTH)),
    y: BOTTOM - point.bytesPerSecond / scaleMaximum.value * (BOTTOM - TOP)
  }))
})
const lastCoordinate = computed(() => coordinates.value.at(-1))
const linePath = computed(() => coordinates.value
  .map((point, index) => `${index === 0 ? 'M' : 'L'} ${point.x.toFixed(1)} ${point.y.toFixed(1)}`)
  .join(' '))
const areaPath = computed(() => {
  const points = coordinates.value
  if (!points.length) return ''
  const first = points[0]
  const last = points.at(-1)
  return first && last ? `${linePath.value} L ${last.x.toFixed(1)} ${BOTTOM} L ${first.x.toFixed(1)} ${BOTTOM} Z` : ''
})
const accessibleLabel = computed(() => t('common.chart.description', {
  current: formatRate(props.currentRate),
  peak: formatRate(peakRate.value)
}))
</script>

<template>
  <figure
    :class="props.compact ? 'h-6 w-full' : 'speed-history border border-muted bg-default p-4'"
    :title="props.compact ? accessibleLabel : undefined"
  >
    <figcaption v-if="!props.compact" class="mb-3 flex flex-wrap items-end justify-between gap-3">
      <div>
        <p class="eyebrow">{{ t('common.chart.title') }}</p>
        <p class="mt-1 text-sm text-muted">{{ t('common.chart.subtitle') }}</p>
      </div>
      <div class="flex items-baseline gap-4 text-right">
        <span class="text-xs text-muted">{{ t('common.chart.peak') }} <span class="numeric text-toned">{{ formatRate(peakRate) }}</span></span>
        <span class="numeric text-lg font-semibold text-highlighted">{{ formatRate(props.currentRate) }}</span>
      </div>
    </figcaption>

    <svg
      :viewBox="`0 0 ${WIDTH} ${HEIGHT}`"
      :class="props.compact ? 'h-full w-full overflow-hidden' : 'h-28 w-full overflow-visible'"
      role="img"
      :aria-label="accessibleLabel"
      preserveAspectRatio="none"
    >
      <defs>
        <linearGradient id="speed-history-area" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stop-color="var(--ui-primary)" stop-opacity="0.28" />
          <stop offset="100%" stop-color="var(--ui-primary)" stop-opacity="0.02" />
        </linearGradient>
      </defs>
      <g v-if="!props.compact" class="text-muted/50" stroke="currentColor" stroke-width="0.6" stroke-dasharray="3 5" vector-effect="non-scaling-stroke">
        <line v-for="ratio in [0, 0.5, 1]" :key="ratio" x1="0" :y1="TOP + (BOTTOM - TOP) * ratio" :x2="WIDTH" :y2="TOP + (BOTTOM - TOP) * ratio" />
      </g>
      <path v-if="areaPath" :d="areaPath" fill="url(#speed-history-area)" />
      <path v-if="linePath" :d="linePath" fill="none" stroke="var(--ui-primary)" :stroke-width="props.compact ? 1.5 : 2" vector-effect="non-scaling-stroke" />
      <circle v-if="lastCoordinate && !props.compact" :cx="lastCoordinate.x" :cy="lastCoordinate.y" r="2.5" fill="var(--ui-primary)" vector-effect="non-scaling-stroke" />
      <line v-if="!props.compact" x1="0" :y1="BOTTOM" :x2="WIDTH" :y2="BOTTOM" stroke="currentColor" class="text-muted" stroke-width="0.8" vector-effect="non-scaling-stroke" />
    </svg>
    <div v-if="!props.compact" class="numeric mt-1 flex justify-between text-[10px] text-muted" aria-hidden="true">
      <span>{{ t('common.chart.window_start') }}</span>
      <span>{{ t('common.chart.window_end') }}</span>
    </div>
  </figure>
</template>
