<script setup lang="ts" generic="T extends WeekWindow">
/**
 * One weekly window — from, to, the days — as the quiet hours, the reconnect windows and the
 * bandwidth schedule edit it (RD-1120-15). The row hands its model back as a new object, so a
 * caller that stores the window inside a settings document replaces it there.
 *
 * `leading` sits before the times (the bandwidth profile), `actions` between the times and the
 * delete button, the default slot under the days.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { clockOf, timeFieldValue, type TimeLike } from '@/utils/timeFields'
import { daysOf, maskOf, minutesOf, timeOf, weekdayItems, type WeekWindow } from '@/utils/weekWindows'

const window = defineModel<T>({ required: true })
const props = defineProps<{ removeLabel?: string }>()
const emit = defineEmits<{ remove: [] }>()
const { t } = useI18n()

const dayItems = computed(() => weekdayItems(t))
const removeLabel = computed(() => props.removeLabel ?? t('common.actions.delete'))

/** A field emptied while it is typed in keeps the stored time; a window has no empty end. */
function setTime(key: 'start_minute' | 'end_minute', value: TimeLike): void {
  const clock = clockOf(value)
  if (clock) window.value = { ...window.value, [key]: minutesOf(clock) }
}

function setDays(days: number[]): void {
  window.value = { ...window.value, days: maskOf(days) }
}
</script>

<template>
  <div class="border border-muted p-3">
    <div class="flex flex-wrap items-end gap-2">
      <slot name="leading" />
      <UFormField :label="t('common.week_window.from')">
        <UInputTime :model-value="timeFieldValue(timeOf(window.start_minute))" class="w-28" @update:model-value="setTime('start_minute', $event)" />
      </UFormField>
      <UFormField :label="t('common.week_window.to')">
        <UInputTime :model-value="timeFieldValue(timeOf(window.end_minute))" class="w-28" @update:model-value="setTime('end_minute', $event)" />
      </UFormField>
      <slot name="actions" />
      <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="removeLabel" :title="removeLabel" @click="emit('remove')" />
    </div>
    <UCheckboxGroup
      class="mt-2"
      :model-value="daysOf(window.days)"
      :items="dayItems"
      :legend="t('common.week_window.days_label')"
      orientation="horizontal"
      size="sm"
      @update:model-value="(days: number[]) => setDays(days)"
    />
    <slot />
  </div>
</template>
