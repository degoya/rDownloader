<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import SpeedHistoryChart from '@/components/SpeedHistoryChart.vue'
import { useTransfersStore } from '@/stores/transfers'
import { formatBytes, formatDuration, formatRate } from '@/utils/format'

const { t } = useI18n()
const transfers = useTransfersStore()
const total = computed(() => formatBytes(transfers.totalCommitted))
const remaining = computed(() => transfers.totalRemaining > 0n ? formatBytes(transfers.totalRemaining) : null)
const volume = computed(() => remaining.value
  ? `${t('downloads.rail.committed', { total: total.value })} · ${t('downloads.rail.remaining', { total: remaining.value })}`
  : t('downloads.rail.committed', { total: total.value }))
const parallelDownloads = computed(() => transfers.downloads.filter(download => download.state === 'downloading').length)
/** Empty while nothing is moving or a size is still unknown — the rail then shows no estimate. */
const etaLabel = computed(() => formatDuration(transfers.queueEta))
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
      <UButton
        v-if="transfers.globalControl"
        :icon="transfers.globalControl === 'pause' ? 'i-lucide-pause' : 'i-lucide-play'"
        size="xs"
        :color="transfers.globalControl === 'pause' ? 'neutral' : 'primary'"
        variant="ghost"
        :aria-label="transfers.globalControl === 'pause' ? t('downloads.header.pause_all') : t('downloads.header.resume_all')"
        :title="transfers.globalControl === 'pause' ? t('downloads.header.pause_all') : t('downloads.header.resume_all')"
        :loading="transfers.controlsBusy"
        @click="transfers.controlAll(transfers.globalControl)"
      />
      <div class="flex shrink-0 items-center gap-1.5" :title="t('downloads.rail.speed')">
        <UIcon name="i-lucide-activity" class="size-3.5 text-primary" />
        <span class="numeric font-semibold text-highlighted">{{ formatRate(transfers.globalRate) }}</span>
      </div>
      <div v-if="etaLabel" class="hidden shrink-0 items-center gap-1.5 text-toned sm:flex" :title="t('downloads.rail.eta_title')">
        <UIcon name="i-lucide-hourglass" class="size-3.5 text-muted" />
        <span class="numeric">{{ t('downloads.rail.eta', { duration: etaLabel }) }}</span>
      </div>
      <div class="w-16 shrink-0 border-x border-muted px-2 sm:w-24">
        <SpeedHistoryChart compact :current-rate="transfers.globalRate" :points="transfers.speedHistory" />
      </div>
      <div class="flex shrink-0 items-center gap-1.5 text-toned" :title="t('downloads.rail.parallel_title')">
        <UIcon name="i-lucide-waypoints" class="size-3.5 text-muted" />
        <span class="numeric"><strong class="text-highlighted">{{ parallelDownloads }}</strong> <span class="hidden @min-[72rem]:inline">{{ t('downloads.rail.parallel') }}</span></span>
      </div>
      <div class="hidden shrink-0 items-center gap-1 sm:flex" :title="t('downloads.toolbar.speed_limit_title')">
        <UIcon name="i-lucide-gauge" class="size-3.5 text-muted" />
        <UInput
          v-model.number="speedInput"
          type="number"
          min="0"
          step="0.5"
          size="xs"
          :placeholder="t('downloads.toolbar.speed_limit_placeholder')"
          class="w-36"
          :ui="{
            base: 'pe-12 font-mono [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none',
            trailing: 'pointer-events-none pe-2'
          }"
          :aria-label="t('downloads.toolbar.speed_limit_aria')"
          @keyup.enter="applySpeedLimit"
        >
          <template #trailing><span class="font-mono text-[10px] text-muted">MiB/s</span></template>
        </UInput>
        <UButton v-if="!speedLimitApplied" size="xs" color="neutral" variant="outline" :label="t('downloads.toolbar.limit')" :loading="transfers.speedLimitBusy" @click="applySpeedLimit" />
        <UButton v-else size="xs" color="neutral" variant="ghost" icon="i-lucide-x" :aria-label="t('downloads.toolbar.clear_limit_aria')" :title="t('downloads.toolbar.clear_limit_title')" :loading="transfers.speedLimitBusy" @click="transfers.setSpeedLimit(null)" />
      </div>
    </div>
    <div class="flex min-w-0 flex-1 items-center justify-end gap-4 text-toned">
      <span class="hidden min-w-0 truncate font-mono md:inline" :title="volume">{{ volume }}</span>
      <span class="flex shrink-0 items-center gap-1 whitespace-nowrap">
        <!-- The name only where the rail has room for it: at 1280 px beside the open sidebar
             it cut the volume line, and the sidebar names the application anyway (RD-120-53). -->
        <span class="hidden font-semibold text-highlighted @min-[72rem]:inline">rDownloader</span>
        <span v-if="serviceVersion" class="font-mono text-muted">v{{ serviceVersion }}</span>
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
