<script setup lang="ts">
/**
 * The day and time a download package may start from (RD-1240-14), opened from the package's
 * menu by `usePackageStartAfter`. The day is the shared `DateField`, the time Nuxt UI's
 * `UInputTime`, both in this browser's time zone; only a moment that lies ahead can be saved,
 * because one that has passed would hold nothing back.
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import DateField from '@/components/DateField.vue'
import { startAfterFields, startAfterMoment } from '@/composables/usePackageStartAfter'
import { formatMoment } from '@/utils/format'
import { clockOf, timeFieldValue, type TimeLike } from '@/utils/timeFields'

const props = defineProps<{
  /** The package's name, for the dialog's description. */
  name: string
  /** The moment set now, if one lies ahead. */
  current: string | null
}>()
const emit = defineEmits<{ close: [result: { at: string } | null] }>()

const { t } = useI18n()
const fields = startAfterFields(props.current)
const day = ref(fields.day)
const clock = ref(fields.clock)
const moment = computed(() => startAfterMoment(day.value, clock.value))
const ahead = computed(() => moment.value !== null && moment.value.getTime() > Date.now())
const today = startAfterFields(new Date().toISOString()).day

function setClock(value: TimeLike): void {
  clock.value = clockOf(value)
}

function submit(): void {
  if (!ahead.value || !moment.value) return
  emit('close', { at: moment.value.toISOString() })
}
</script>

<template>
  <UModal :title="t('downloads.start_after.title')" :description="t('downloads.start_after.description', { name: props.name })" :close="{ onClick: () => emit('close', null) }">
    <template #body>
      <form id="package-start-after-form" class="grid gap-3 sm:grid-cols-2" @submit.prevent="submit">
        <UFormField :label="t('downloads.start_after.day')">
          <DateField v-model="day" :min="today" class="w-full" data-testid="start-after-day" />
        </UFormField>
        <UFormField :label="t('downloads.start_after.time')">
          <UInputTime :model-value="timeFieldValue(clock)" class="w-full" data-testid="start-after-time" @update:model-value="setClock" />
        </UFormField>
        <p class="text-sm sm:col-span-2" :class="ahead ? 'text-muted' : 'text-error'" data-testid="start-after-summary">
          {{ ahead && moment ? t('downloads.start_after.glyph_title', { time: formatMoment(moment.toISOString()) }) : t('downloads.start_after.not_ahead') }}
        </p>
      </form>
    </template>
    <template #footer>
      <UButton :label="t('common.actions.cancel')" color="neutral" variant="outline" @click="emit('close', null)" />
      <UButton :label="t('common.actions.save')" icon="i-lucide-save" type="submit" form="package-start-after-form" :disabled="!ahead" data-testid="start-after-save" />
    </template>
  </UModal>
</template>
