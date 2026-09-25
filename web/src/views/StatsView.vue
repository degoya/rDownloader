<script setup lang="ts">
/**
 * The statistics view (RD-110-01): what the service transferred, over a chosen range.
 *
 * Everything on it is read from `/api/v1/stats/transfers`; nothing is computed from the live
 * queue, so the page says the same thing after a restart. The scrape card at the bottom
 * names the metrics route because the token editor is the other half of that feature and
 * lives two pages away.
 */
import { computed, onMounted, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'

import type { StatsRange, TransferStatsGroup } from '@/api/types'
import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import TransferStatsChart from '@/components/TransferStatsChart.vue'
import { useStatsStore } from '@/stores/stats'
import { formatBytes, formatDuration } from '@/utils/format'

const { t, n } = useI18n()
const store = useStatsStore()

const RANGES: StatsRange[] = ['day', 'week', 'month', 'year']

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

const endpoint = computed(() => `${window.location.origin}/api/v1/metrics`)
</script>

<template>
  <UDashboardPanel id="stats">
    <template #header>
      <UDashboardNavbar :title="t('stats.title')">
        <template #leading><UDashboardSidebarCollapse /></template>
        <template #right>
          <div class="flex gap-1" role="group" :aria-label="t('stats.ranges.label')">
            <UButton
              v-for="range in RANGES"
              :key="range"
              size="sm"
              :color="store.range === range ? 'primary' : 'neutral'"
              :variant="store.range === range ? 'solid' : 'subtle'"
              :aria-pressed="store.range === range"
              :label="t(`stats.ranges.${range}`)"
              @click="store.setRange(range)"
            />
          </div>
        </template>
      </UDashboardNavbar>
    </template>
    <template #body>
      <div class="w-full space-y-4">
        <section class="border border-muted bg-default p-5">
          <SectionHeader level="page" :eyebrow="t('stats.eyebrow')" :title="t('stats.title')" :description="t('stats.description')" />
          <UAlert v-if="store.error" class="mt-4" color="error" variant="subtle" role="alert" :description="store.error" />
          <DataState :loading="store.loading" :error="null" :empty="empty" :rows="2" variant="inline" class="mt-4">
            <p class="text-sm text-muted">{{ t('stats.chart.empty') }}</p>
          </DataState>
          <div v-if="tiles.length" class="mt-4 grid gap-px border border-muted bg-muted sm:grid-cols-3 xl:grid-cols-6">
            <div v-for="tile in tiles" :key="tile.key" class="bg-elevated p-4" :data-tile="tile.key">
              <p class="eyebrow">{{ tile.label }}</p>
              <p class="numeric mt-2 text-lg text-highlighted">{{ tile.value }}</p>
              <p class="mt-1 text-xs text-muted">{{ tile.hint }}</p>
            </div>
          </div>
          <TransferStatsChart v-if="store.stats?.buckets.length" class="mt-3" :buckets="store.stats.buckets" :resolution="store.stats.resolution" :since="store.stats.since" />
        </section>

        <div v-if="store.stats?.buckets.length" class="grid gap-4 lg:grid-cols-2">
          <section v-for="group in (['kind', 'provider'] as const)" :key="group" class="border border-muted bg-default p-5">
            <SectionHeader :eyebrow="t('stats.eyebrow')" :title="group === 'kind' ? t('stats.groups.by_kind') : t('stats.groups.by_provider')" />
            <table class="mt-3 w-full text-sm">
              <thead>
                <tr class="text-left text-xs text-muted">
                  <th class="pb-2 font-medium">{{ t('stats.groups.key') }}</th>
                  <th class="pb-2 text-right font-medium">{{ t('stats.groups.completed') }}</th>
                  <th class="pb-2 text-right font-medium">{{ t('stats.groups.failed') }}</th>
                  <th class="pb-2 text-right font-medium">{{ t('stats.groups.retries') }}</th>
                  <th class="pb-2 text-right font-medium">{{ t('stats.groups.bytes') }}</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="row in (group === 'kind' ? store.stats.by_kind : store.stats.by_provider)" :key="row.key" class="border-t border-muted">
                  <td class="py-2 font-mono text-xs text-highlighted">{{ groupKey(row, group) }}</td>
                  <td class="numeric py-2 text-right">{{ n(row.completed) }}</td>
                  <td class="numeric py-2 text-right">{{ n(row.failed) }}</td>
                  <td class="numeric py-2 text-right">{{ n(row.retries) }}</td>
                  <td class="numeric py-2 text-right">{{ formatBytes(String(row.bytes)) }}</td>
                </tr>
              </tbody>
            </table>
          </section>
        </div>

        <section class="border border-muted bg-default p-5">
          <SectionHeader :eyebrow="t('stats.metrics.eyebrow')" :title="t('stats.metrics.title')" :description="t('stats.metrics.description')" />
          <p class="mt-3 text-xs text-muted">{{ t('stats.metrics.endpoint') }}</p>
          <p class="mt-1 font-mono text-sm text-highlighted">{{ endpoint }}</p>
          <p class="mt-2 text-xs text-muted">{{ t('stats.metrics.scope_hint') }}</p>
        </section>
      </div>
    </template>
  </UDashboardPanel>
</template>
