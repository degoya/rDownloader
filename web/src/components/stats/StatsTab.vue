<script setup lang="ts">
/**
 * The statistics (RD-110-01): what the service transferred, over a chosen range — the first tab
 * of the statistics and history page (RD-1101-05, `StatsHistoryView.vue`).
 *
 * Everything on it is read from `/api/v1/stats/transfers`; nothing is computed from the live
 * queue, so the page says the same thing after a restart. The scrape card at the bottom
 * names the metrics route because the token editor is the other half of that feature and
 * lives two pages away.
 */
import { computed, onMounted, onUnmounted } from 'vue'
import type { TableColumn } from '@nuxt/ui'
import { useI18n } from 'vue-i18n'

import type { StatsRange, TransferStatsGroup, UsenetServerTraffic } from '@/api/types'
import { serviceUrl } from '@/basePath'
import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import StatTiles from '@/components/StatTiles.vue'
import TransferStatsChart from '@/components/TransferStatsChart.vue'
import { useStatsStore } from '@/stores/stats'
import { formatBytes, formatDuration } from '@/utils/format'

const { t, n } = useI18n()
const store = useStatsStore()

const RANGES: StatsRange[] = ['day', 'week', 'month', 'year']
const rangeItems = computed(() => RANGES.map(value => ({ label: t(`stats.ranges.${value}`), value })))

onMounted(() => store.start())
onUnmounted(() => store.stop())

const totals = computed(() => store.stats?.totals)
const empty = computed(() => store.settled && !store.error && !(store.stats?.buckets.length))

const tiles = computed(() => {
  const figures = totals.value
  const allTime = store.stats?.all_time
  if (!figures || !allTime) return []
  const turnaround = figures.completed > 0 ? Number(figures.seconds) / figures.completed : null
  // Transfers are timed to the whole second, so a range of quick ones means 0 s, which
  // `formatDuration` draws as nothing at all: the tile looked broken beside six completions
  // (RD-120-48). Under a second is a figure, and it says so.
  const turnaroundLabel = turnaround === null ? '—' : turnaround < 1 ? t('stats.tiles.turnaround_under_second') : formatDuration(turnaround)
  return [
    { key: 'completed', label: t('stats.tiles.completed'), value: n(figures.completed), hint: t('stats.tiles.completed_hint') },
    { key: 'failed', label: t('stats.tiles.failed'), value: n(figures.failed), hint: t('stats.tiles.failed_hint') },
    { key: 'retries', label: t('stats.tiles.retries'), value: n(figures.retries), hint: t('stats.tiles.retries_hint') },
    { key: 'bytes', label: t('stats.tiles.bytes'), value: formatBytes(String(figures.bytes)), hint: t('stats.tiles.bytes_hint') },
    { key: 'turnaround', label: t('stats.tiles.turnaround'), value: turnaroundLabel, hint: t('stats.tiles.turnaround_hint') },
    { key: 'all_time', label: t('stats.tiles.all_time'), value: formatBytes(String(allTime.bytes)), hint: t('stats.tiles.all_time_hint', { completed: n(allTime.completed) }) }
  ]
})

function groupKey(group: TransferStatsGroup, kind: 'kind' | 'provider'): string {
  return kind === 'provider' && group.key === 'direct' ? t('stats.groups.direct') : group.key
}

/** The two breakdowns share one table shape; the figures sit right-aligned, as figures do. */
const FIGURE_CELL = { th: 'text-right', td: 'numeric text-right' }
const groupColumns = computed<TableColumn<TransferStatsGroup>[]>(() => [
  { id: 'key', header: t('stats.groups.key') },
  { id: 'completed', header: t('stats.groups.completed'), meta: { class: FIGURE_CELL } },
  { id: 'failed', header: t('stats.groups.failed'), meta: { class: FIGURE_CELL } },
  { id: 'retries', header: t('stats.groups.retries'), meta: { class: FIGURE_CELL } },
  { id: 'bytes', header: t('stats.groups.bytes'), meta: { class: FIGURE_CELL } }
])

/** The traffic per Usenet server (RD-1100-05): fixed ranges, whichever one the page shows. */
const SERVER_FIGURES = ['today', 'week', 'month', 'year', 'total'] as const
const serverColumns = computed<TableColumn<UsenetServerTraffic>[]>(() => [
  { id: 'name', header: t('stats.servers.name') },
  ...SERVER_FIGURES.map(id => ({ id, header: t(`stats.servers.${id}`), meta: { class: FIGURE_CELL } })),
  { id: 'quota', header: t('stats.servers.quota'), meta: { class: FIGURE_CELL } }
])

function quotaLabel(server: UsenetServerTraffic): string {
  const quota = server.quota
  if (!quota) return '—'
  return t('stats.servers.quota_used', { used: formatBytes(String(quota.used_bytes)), limit: formatBytes(String(quota.limit_bytes)) })
}

const endpoint = computed(() => serviceUrl('/api/v1/metrics'))
</script>

<template>
  <div class="w-full space-y-4">
    <UCard as="section">
      <!-- The ranges sat in the navbar while this was a page of its own; under the shared page's
           tabs (RD-1101-05) they belong to this tab, so they stand beside its heading and wrap
           below it on a phone. -->
      <div class="flex flex-wrap items-start justify-between gap-4">
        <SectionHeader level="page" :eyebrow="t('stats.eyebrow')" :title="t('stats.title')" :description="t('stats.description')" />
        <URadioGroup
          :model-value="store.range"
          :items="rangeItems"
          variant="card"
          indicator="hidden"
          orientation="horizontal"
          size="xs"
          :aria-label="t('stats.ranges.label')"
          @update:model-value="store.setRange"
        />
      </div>
      <UAlert v-if="store.error" class="mt-4" color="error" role="alert" :description="store.error" />
      <DataState :loading="store.loading" :error="null" :empty="empty" :rows="2" variant="inline" class="mt-4">
        <UEmpty :description="t('stats.chart.empty')" />
      </DataState>
      <StatTiles v-if="tiles.length" :tiles="tiles" class="mt-4 sm:grid-cols-3 xl:grid-cols-6" />
      <TransferStatsChart v-if="store.stats?.buckets.length" class="mt-3" :buckets="store.stats.buckets" :resolution="store.stats.resolution" :since="store.stats.since" />
    </UCard>

    <div v-if="store.stats?.buckets.length" class="grid gap-4 lg:grid-cols-2">
      <UCard v-for="group in (['kind', 'provider'] as const)" :key="group" as="section">
        <SectionHeader :eyebrow="t('stats.eyebrow')" :title="group === 'kind' ? t('stats.groups.by_kind') : t('stats.groups.by_provider')" />
        <UTable
          class="mt-3"
          :data="group === 'kind' ? store.stats.by_kind : store.stats.by_provider"
          :columns="groupColumns"
          :ui="{ th: 'px-0 py-2 pr-3 text-xs font-medium text-muted last:pr-0', td: 'px-0 py-2 pr-3 text-sm last:pr-0' }"
        >
          <template #key-cell="{ row }"><span class="font-mono text-xs text-highlighted">{{ groupKey(row.original, group) }}</span></template>
          <template #completed-cell="{ row }">{{ n(row.original.completed) }}</template>
          <template #failed-cell="{ row }">{{ n(row.original.failed) }}</template>
          <template #retries-cell="{ row }">{{ n(row.original.retries) }}</template>
          <template #bytes-cell="{ row }">{{ formatBytes(String(row.original.bytes)) }}</template>
        </UTable>
      </UCard>
    </div>

    <UCard v-if="store.servers.length" as="section" data-stats="usenet-servers">
      <SectionHeader :eyebrow="t('stats.eyebrow')" :title="t('stats.servers.title')" :description="t('stats.servers.description')" />
      <UTable
        class="mt-3"
        :data="store.servers"
        :columns="serverColumns"
        :ui="{ th: 'px-0 py-2 pr-3 text-xs font-medium text-muted last:pr-0', td: 'px-0 py-2 pr-3 text-sm last:pr-0' }"
      >
        <template #name-cell="{ row }"><span class="text-highlighted" :class="row.original.enabled ? '' : 'text-muted'">{{ row.original.name }}</span></template>
        <template #today-cell="{ row }">{{ formatBytes(String(row.original.today)) }}</template>
        <template #week-cell="{ row }">{{ formatBytes(String(row.original.week)) }}</template>
        <template #month-cell="{ row }">{{ formatBytes(String(row.original.month)) }}</template>
        <template #year-cell="{ row }">{{ formatBytes(String(row.original.year)) }}</template>
        <template #total-cell="{ row }">{{ formatBytes(String(row.original.total)) }}</template>
        <template #quota-cell="{ row }">
          <span>{{ quotaLabel(row.original) }}</span>
          <UBadge v-if="row.original.quota?.reached_at" class="ml-2" color="warning" variant="subtle" size="sm">
            {{ row.original.quota.action === 'pause' ? t('stats.servers.used_up_pause') : t('stats.servers.used_up_backup') }}
          </UBadge>
        </template>
      </UTable>
    </UCard>

    <UCard as="section">
      <SectionHeader :eyebrow="t('stats.metrics.eyebrow')" :title="t('stats.metrics.title')" :description="t('stats.metrics.description')" />
      <p class="mt-3 text-xs text-muted">{{ t('stats.metrics.endpoint') }}</p>
      <p class="mt-1 font-mono text-sm text-highlighted">{{ endpoint }}</p>
      <p class="mt-2 text-xs text-muted">{{ t('stats.metrics.scope_hint') }}</p>
    </UCard>
  </div>
</template>
