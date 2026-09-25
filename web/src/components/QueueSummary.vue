<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DownloadSummary } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SpeedHistoryChart from '@/components/SpeedHistoryChart.vue'
import { formatBytes, formatDuration } from '@/utils/format'
import type { TransferRateHistoryPoint } from '@/utils/transferRates'

const props = defineProps<{
  summary: DownloadSummary
  currentRate: number
  /** Seconds until the queue is through, or `null` when no honest figure exists. */
  etaSeconds: number | null
  speedHistory: TransferRateHistoryPoint[]
}>()
const { t } = useI18n()

const tiles = computed(() => [
  { label: t('downloads.summary.queued'), value: props.summary.queued.toString(), hint: t('downloads.summary.queued_hint') },
  { label: t('downloads.summary.active'), value: props.summary.active.toString(), hint: t('downloads.summary.active_hint') },
  { label: t('downloads.summary.paused_blocked'), value: `${props.summary.paused} / ${props.summary.blocked}`, hint: t('downloads.summary.paused_blocked_hint') },
  { label: t('downloads.summary.completed_failed'), value: `${props.summary.completed} / ${props.summary.failed}`, hint: t('downloads.summary.completed_failed_hint') },
  { label: t('downloads.summary.downloaded'), value: formatBytes(props.summary.committed_bytes), hint: t('downloads.summary.downloaded_hint', { total: formatBytes(props.summary.total_bytes) }) },
  { label: t('downloads.summary.remaining'), value: formatBytes(props.summary.remaining_bytes), hint: t('downloads.summary.remaining_hint') },
  // An em dash rather than a number: without a rate, or with a size still unknown, there is
  // nothing to estimate from and inventing a figure would be worse than saying nothing.
  { label: t('downloads.summary.eta'), value: formatDuration(props.etaSeconds) || '—', hint: t('downloads.summary.eta_hint') }
])

function usage(free: string | null | undefined, total: string | null | undefined): number {
  if (!free || !total || BigInt(total) === 0n) return 0
  return Number((BigInt(total) - BigInt(free)) * 100n / BigInt(total))
}
</script>

<template>
  <section class="border border-muted bg-elevated p-4">
    <div class="mb-3">
      <SectionHeader :eyebrow="t('downloads.summary.eyebrow')" :title="t('downloads.summary.title')" />
    </div>
    <div class="grid gap-px border border-muted bg-muted sm:grid-cols-3 xl:grid-cols-7">
      <div v-for="tile in tiles" :key="tile.label" class="bg-elevated p-4">
        <p class="eyebrow">{{ tile.label }}</p>
        <p class="numeric mt-2 text-lg text-highlighted">{{ tile.value }}</p>
        <p class="mt-1 text-xs text-muted">{{ tile.hint }}</p>
      </div>
    </div>
    <SpeedHistoryChart class="mt-3" :current-rate="props.currentRate" :points="props.speedHistory" />
    <div class="mt-3 grid gap-2 md:grid-cols-2">
      <div v-for="root in props.summary.storage" :key="root.id" class="border border-muted bg-default p-3">
        <div class="flex items-center justify-between gap-3">
          <div class="min-w-0">
            <p class="truncate text-sm font-medium text-highlighted">{{ root.name }}<span v-if="root.is_default" class="ml-2 text-xs text-muted">{{ t('common.values.default') }}</span></p>
            <p class="truncate font-mono text-[11px] text-muted">{{ root.path }}</p>
          </div>
          <p class="numeric shrink-0 text-sm text-highlighted">
            <template v-if="root.free_bytes">{{ t('downloads.summary.free', { value: formatBytes(root.free_bytes) }) }}</template>
            <template v-else><span class="text-warning">{{ t('downloads.summary.unreachable') }}</span></template>
          </p>
        </div>
        <UProgress v-if="root.total_bytes" :model-value="usage(root.free_bytes, root.total_bytes)" size="xs" class="mt-2" :color="usage(root.free_bytes, root.total_bytes) > 90 ? 'error' : 'primary'" />
        <p v-if="root.total_bytes" class="mt-1 text-xs text-muted">{{ t('downloads.summary.used', { percent: usage(root.free_bytes, root.total_bytes), total: formatBytes(root.total_bytes) }) }}</p>
      </div>
    </div>
  </section>
</template>
