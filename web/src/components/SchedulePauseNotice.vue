<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { useQueuePauseStore } from '@/stores/queuePause'
import { formatPauseEnd } from '@/utils/format'

/**
 * A bandwidth profile that pauses downloads (RD-1240-30), beside the queue's pause control: the
 * profile, until when, and in the header the way to download anyway — switching to another
 * profile on the bandwidth page, which ends it as the switch by hand always has.
 */
const props = withDefaults(defineProps<{
  /** `rail`: the badge only, its text in the tooltip; `header`: the text beside it. */
  placement?: 'header' | 'rail'
}>(), { placement: 'header' })

const { t } = useI18n()
const queuePause = useQueuePauseStore()

const pause = computed(() => queuePause.schedulePause)
const label = computed(() => pause.value?.until
  ? t('downloads.schedule_pause.label', { time: formatPauseEnd(pause.value.until) })
  : t('downloads.schedule_pause.label_open'))
const title = computed(() => pause.value
  ? `${label.value}\n${t('downloads.schedule_pause.title', { profile: pause.value.profile_name })}`
  : '')
</script>

<template>
  <div v-if="pause" class="flex min-w-0 items-center gap-1" data-testid="schedule-pause-notice">
    <UBadge
      icon="i-lucide-calendar-clock"
      color="warning"
      variant="subtle"
      :size="props.placement === 'rail' ? 'sm' : 'md'"
      class="min-w-0"
      :label="props.placement === 'rail' ? undefined : label"
      :aria-label="label"
      :title="title"
      :ui="{ label: 'truncate' }"
    />
    <UButton
      v-if="props.placement === 'header'"
      icon="i-lucide-arrow-right-left"
      size="xs"
      color="neutral"
      variant="ghost"
      :label="t('downloads.schedule_pause.open_settings')"
      :ui="{ label: 'max-sm:hidden' }"
      to="/settings/bandwidth?tab=status"
      data-testid="schedule-pause-switch"
    />
  </div>
</template>
