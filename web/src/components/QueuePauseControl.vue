<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { nextOccurrence, PAUSE_DURATIONS, useQueuePauseStore } from '@/stores/queuePause'
import { useTransfersStore } from '@/stores/transfers'
import { formatDuration, formatPauseEnd } from '@/utils/format'

/**
 * The global start/pause control with its timed pause (RD-190-20), in the Downloads header and
 * on the transfer rail.
 *
 * Unpaused, it is the toggle it always was — pause or start everything — with a menu beside it
 * that pauses for 30 minutes, an hour, three hours or until a time. While a timed pause holds,
 * the control says until when and resumes everything at a click, which also ends the pause.
 */
const props = withDefaults(defineProps<{
  /** `rail`: icon-sized, the labels in the tooltip; `header`: labelled buttons. */
  placement?: 'header' | 'rail'
}>(), { placement: 'header' })

const { t } = useI18n()
const transfers = useTransfersStore()
const queuePause = useQueuePauseStore()
const compact = computed(() => props.placement === 'rail')
const size = computed(() => compact.value ? 'xs' as const : 'md' as const)
// A phone keeps the header's icons only; the names stay as `aria-label` and `title`, and the rail
// still says until when a timed pause holds.
const labelUi = { label: 'max-sm:hidden' }

const untilOpen = ref(false)
const untilClock = ref('')
const untilAt = computed(() => nextOccurrence(untilClock.value))

const DURATION_KEYS: Record<(typeof PAUSE_DURATIONS)[number], string> = {
  30: 'downloads.pause.for_30m',
  60: 'downloads.pause.for_1h',
  180: 'downloads.pause.for_3h'
}

const items = computed(() => [
  PAUSE_DURATIONS.map(minutes => ({
    label: t(DURATION_KEYS[minutes]),
    icon: 'i-lucide-timer',
    onSelect: () => { void pauseWith(() => queuePause.pauseFor(minutes)) }
  })),
  [{ label: t('downloads.pause.until'), icon: 'i-lucide-clock', onSelect: () => openUntil() }]
])

const endLabel = computed(() => formatPauseEnd(queuePause.until))
const remainingTitle = computed(() =>
  t('downloads.pause.remaining_title', { duration: formatDuration(queuePause.remainingSeconds) }))
const globalLabel = computed(() =>
  transfers.globalControl === 'pause' ? t('downloads.header.pause_all') : t('downloads.header.resume_all'))

function openUntil(): void {
  untilClock.value = ''
  untilOpen.value = true
}

async function pauseWith(action: () => Promise<boolean>): Promise<void> {
  if (await action()) {
    transfers.notice = t('downloads.pause.notice_paused', { time: formatPauseEnd(queuePause.until) })
    await transfers.refresh()
  } else {
    transfers.error = queuePause.error
  }
}

async function confirmUntil(): Promise<void> {
  const at = untilAt.value
  if (!at) return
  untilOpen.value = false
  await pauseWith(() => queuePause.pauseUntil(at))
}

async function resumeNow(): Promise<void> {
  const resumed = await queuePause.resume()
  if (resumed === null) {
    transfers.error = queuePause.error
    return
  }
  transfers.notice = t('downloads.notices.resumed_count', { count: resumed }, resumed)
  await transfers.refresh()
}
</script>

<template>
  <div class="flex shrink-0 items-center gap-1" data-testid="queue-pause-control">
    <template v-if="queuePause.active">
      <UButton
        icon="i-lucide-play"
        :size="size"
        color="primary"
        :variant="compact ? 'ghost' : 'soft'"
        :label="compact ? undefined : t('downloads.pause.paused_until', { time: endLabel })"
        :aria-label="t('downloads.pause.resume_now')"
        :ui="labelUi"
        :title="remainingTitle"
        :loading="queuePause.busy"
        data-testid="queue-pause-resume"
        @click="resumeNow"
      />
      <span v-if="compact" class="numeric whitespace-nowrap text-primary" :title="remainingTitle">
        {{ t('downloads.pause.paused_until', { time: endLabel }) }}
      </span>
    </template>
    <template v-else>
      <UButton
        v-if="transfers.globalControl"
        :icon="transfers.globalControl === 'pause' ? 'i-lucide-pause' : 'i-lucide-play'"
        :size="size"
        :label="compact ? undefined : globalLabel"
        :color="transfers.globalControl === 'pause' ? 'neutral' : 'primary'"
        :variant="compact ? 'ghost' : transfers.globalControl === 'pause' ? 'outline' : 'soft'"
        :aria-label="globalLabel"
        :title="globalLabel"
        :ui="labelUi"
        :loading="transfers.controlsBusy"
        @click="transfers.controlAll(transfers.globalControl)"
      />
      <UDropdownMenu :items="items">
        <UButton
          icon="i-lucide-timer"
          trailing-icon="i-lucide-chevron-down"
          :size="size"
          color="neutral"
          :variant="compact ? 'ghost' : 'outline'"
          :aria-label="t('downloads.pause.menu_aria')"
          :title="t('downloads.pause.menu_aria')"
          :loading="queuePause.busy"
        />
      </UDropdownMenu>
    </template>
    <UModal v-model:open="untilOpen" :title="t('downloads.pause.until_title')" :description="t('downloads.pause.until_hint')" :ui="{ footer: 'justify-end' }">
      <template #body>
        <UFormField :label="t('downloads.pause.until_time')">
          <UInput v-model="untilClock" type="time" class="w-32" data-testid="queue-pause-until" @keyup.enter="confirmUntil" />
        </UFormField>
      </template>
      <template #footer>
        <UButton :label="t('common.actions.cancel')" color="neutral" variant="outline" @click="untilOpen = false" />
        <UButton icon="i-lucide-pause" :label="t('downloads.pause.confirm')" :disabled="!untilAt" @click="confirmUntil" />
      </template>
    </UModal>
  </div>
</template>
