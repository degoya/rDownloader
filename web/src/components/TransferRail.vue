<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import QueuePauseControl from '@/components/QueuePauseControl.vue'
import SpeedHistoryChart from '@/components/SpeedHistoryChart.vue'
import { useSelectionStore } from '@/stores/selection'
import { useTransfersStore } from '@/stores/transfers'
import { formatBytes, formatDuration, formatRate } from '@/utils/format'
import { DECIMAL } from '@/utils/numberInput'

const { t } = useI18n()
const transfers = useTransfersStore()
const selection = useSelectionStore()
const total = computed(() => formatBytes(transfers.totalCommitted))
const remaining = computed(() => transfers.totalRemaining > 0n ? formatBytes(transfers.totalRemaining) : null)
const volume = computed(() => remaining.value
  ? `${t('downloads.rail.committed', { total: total.value })} · ${t('downloads.rail.remaining', { total: remaining.value })}`
  : t('downloads.rail.committed', { total: total.value }))
const parallelDownloads = computed(() => transfers.downloads.filter(download => download.state === 'downloading').length)
/** Empty while nothing is moving or a size is still unknown — the rail then shows no estimate. */
const etaLabel = computed(() => formatDuration(transfers.queueEta))
/**
 * What the open list has ticked (RD-170-14). A sum that leaves out a size nobody knows yet is a
 * lower bound and says so with `≥`; with no size known at all there is only the count.
 */
const selectionSize = computed(() => {
  const size = selection.size
  if (!size || size.unknown === size.count) return null
  return size.unknown ? `≥ ${formatBytes(size.bytes)}` : formatBytes(size.bytes)
})
const selectionTitle = computed(() => {
  const size = selection.size
  if (!size) return ''
  if (size.unknown === size.count) return t('downloads.rail.selection_unknown_title', { count: size.count })
  const params = { count: size.count, size: formatBytes(size.bytes), unknown: size.unknown }
  return t(size.unknown ? 'downloads.rail.selection_partial_title' : 'downloads.rail.selection_title', params)
})
const serviceVersion = ref('')

const speedInput = ref<number | null>(null)
watch(() => transfers.speedLimitMiB, (value) => { speedInput.value = value }, { immediate: true })
const speedLimitApplied = computed(() => {
  if (!transfers.speedLimitMiB || !speedInput.value) return false
  return Math.abs(transfers.speedLimitMiB - speedInput.value) < 0.001
})

async function applySpeedLimit(): Promise<void> {
  await transfers.setSpeedLimit(speedInput.value && speedInput.value > 0 ? speedInput.value : null)
}

onMounted(async () => {
  void transfers.loadSpeedLimit()
  const response = await api.GET('/api/v1/health')
  const data = response.data as { version?: string } | undefined
  if (data?.version) serviceVersion.value = data.version
})
</script>

<template>
  <!--
    The rail's width is the panel's, not the window's: with the sidebar open a 1440 px window
    leaves some 1220 px, and viewport breakpoints then let the right half run under the speed
    limit field and break the signature over two lines (RD-120-48). So the controls on the left
    keep their size, the right half takes what is left and truncates rather than overlapping,
    and the signature appears only once the rail itself is wide enough (a container query).
  -->
  <footer data-tour="rail" class="@container relative z-20 flex h-11 shrink-0 items-center justify-between gap-4 border-t border-muted bg-elevated px-4 text-xs sm:px-6">
    <div class="flex shrink-0 items-center gap-2 sm:gap-3">
      <QueuePauseControl placement="rail" />
      <div class="flex shrink-0 items-center gap-1.5" :title="t('downloads.rail.speed')">
        <UIcon name="i-lucide-activity" class="size-3.5 text-primary" />
        <span class="numeric font-semibold text-highlighted">{{ formatRate(transfers.globalRate) }}</span>
      </div>
      <div v-if="etaLabel" class="hidden shrink-0 items-center gap-1.5 text-toned sm:flex" :title="t('downloads.rail.eta_title')">
        <UIcon name="i-lucide-hourglass" class="size-3.5 text-muted" />
        <span class="numeric">{{ t('downloads.rail.eta', { duration: etaLabel }) }}</span>
      </div>
      <!-- On a phone the rail has no room for the chart beside a timed pause's "paused until",
           and the version wrote over the connection count; both wait for a wider rail. -->
      <div class="hidden w-16 shrink-0 border-x border-muted px-2 @min-[26rem]:block sm:w-24">
        <SpeedHistoryChart compact :current-rate="transfers.globalRate" :points="transfers.speedHistory" />
      </div>
      <div class="flex shrink-0 items-center gap-1.5 text-toned" :title="t('downloads.rail.parallel_title')">
        <UIcon name="i-lucide-waypoints" class="size-3.5 text-muted" />
        <span class="numeric"><strong class="text-highlighted">{{ parallelDownloads }}</strong> <span class="hidden @min-[72rem]:inline">{{ t('downloads.rail.parallel') }}</span></span>
      </div>
      <div class="hidden shrink-0 items-center gap-1 sm:flex" :title="t('downloads.toolbar.speed_limit_title')">
        <UIcon name="i-lucide-gauge" class="size-3.5 text-muted" />
        <UFieldGroup size="xs" class="w-36">
          <UInputNumber
            v-model="speedInput"
            :min="0"
            :step="0.5"
            :step-snapping="false"
            :format-options="DECIMAL"
            :placeholder="t('downloads.toolbar.speed_limit_placeholder')"
            :ui="{ base: 'font-mono' }"
            :aria-label="t('downloads.toolbar.speed_limit_aria')"
            @keyup.enter="applySpeedLimit"
          />
          <UBadge color="neutral" variant="outline" label="MiB/s" class="font-mono" />
        </UFieldGroup>
        <UButton v-if="!speedLimitApplied" size="xs" color="neutral" variant="outline" :label="t('downloads.toolbar.limit')" :loading="transfers.speedLimitBusy" @click="applySpeedLimit" />
        <UButton v-else size="xs" color="neutral" variant="ghost" icon="i-lucide-x" :aria-label="t('downloads.toolbar.clear_limit_aria')" :title="t('downloads.toolbar.clear_limit_title')" :loading="transfers.speedLimitBusy" @click="transfers.setSpeedLimit(null)" />
      </div>
    </div>
    <div class="flex min-w-0 flex-1 items-center justify-end gap-4 text-toned">
      <!-- Short on a narrow rail: the count and the size, the word only once there is room. -->
      <span v-if="selection.size" data-testid="rail-selection" class="flex min-w-0 items-center gap-1.5" :title="selectionTitle">
        <UIcon name="i-lucide-list-checks" class="size-3.5 shrink-0 text-primary" />
        <span class="numeric truncate">
          <strong class="text-highlighted">{{ selection.size.count }}</strong>
          <span class="hidden @min-[40rem]:inline">&nbsp;{{ t('common.units.selected') }}</span>
          <template v-if="selectionSize"> · {{ selectionSize }}</template>
        </span>
      </span>
      <span class="hidden min-w-0 truncate font-mono md:inline" :title="volume">{{ volume }}</span>
      <span class="flex shrink-0 items-center gap-1 whitespace-nowrap">
        <!-- The name only where the rail has room for it: at 1280 px beside the open sidebar
             it cut the volume line, and the sidebar names the application anyway (RD-120-53). -->
        <span class="hidden font-semibold text-highlighted @min-[72rem]:inline">rDownloader</span>
        <span v-if="serviceVersion" class="hidden font-mono text-muted @min-[48rem]:inline">v{{ serviceVersion }}</span>
        <span class="hidden items-center gap-1 @min-[96rem]:flex">
          —
          {{ t('common.footer.made_with') }}
          <UIcon name="i-lucide-heart" class="size-3 text-primary" />
          {{ t('common.footer.by', { author: 'Alexander Herling' }) }}
        </span>
      </span>
    </div>
    <div class="transfer-stripe absolute inset-x-0 top-0 h-px" />
  </footer>
</template>
