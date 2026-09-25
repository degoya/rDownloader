<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { BandwidthProfile, BandwidthSchedule, BandwidthScheduleRequest } from '@/api/types'
import FormListLayout from '@/components/FormListLayout.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import TimezoneSelect from '@/components/TimezoneSelect.vue'
import { NO_SELECTION, optionalSelection, selectionValue } from '@/utils/select'

const props = defineProps<{ profiles: BandwidthProfile[] }>()
const schedule = defineModel<BandwidthSchedule>({ required: true })
const emit = defineEmits<{ changed: [] }>()
const { t } = useI18n()
const pending = ref(false)
const error = ref<string | null>(null)
const message = ref<string | null>(null)

/** Monday-first, matching the backend's bitmask. */
const DAYS = [0, 1, 2, 3, 4, 5, 6]

interface WindowDraft {
  profile_id: string
  days: number
  start_minute: number
  end_minute: number
  priority: number
  enabled: boolean
}

const windows = ref<WindowDraft[]>(
  schedule.value.windows.map(window => ({
    profile_id: window.profile_id,
    days: window.days,
    start_minute: window.start_minute,
    end_minute: window.end_minute,
    priority: window.priority,
    enabled: window.enabled
  }))
)

/** `null` is not a legal select value in Reka UI; map it through the shared sentinel. */
const defaultProfileChoice = computed({
  get: () => optionalSelection(schedule.value.default_profile_id),
  set: (value: string) => { schedule.value.default_profile_id = selectionValue(value) }
})
const profileItems = computed(() =>
  props.profiles.map(profile => ({ value: profile.id, label: profile.name }))
)

/** `HH:MM` in the schedule's own timezone; the backend stores minutes since midnight. */
function timeOf(minutes: number): string {
  const hours = Math.floor(minutes / 60)
  return `${String(hours).padStart(2, '0')}:${String(minutes % 60).padStart(2, '0')}`
}

function minutesOf(value: string): number {
  const [hours, minutes] = value.split(':').map(Number)
  const total = (hours ?? 0) * 60 + (minutes ?? 0)
  return Number.isFinite(total) ? Math.min(Math.max(total, 0), 1440) : 0
}

function toggleDay(window: WindowDraft, day: number): void {
  window.days ^= 1 << day
}

function addWindow(): void {
  const first = props.profiles[0]
  if (!first) return
  windows.value = [
    ...windows.value,
    { profile_id: first.id, days: 0b0111_1111, start_minute: 22 * 60, end_minute: 6 * 60, priority: 0, enabled: true }
  ]
}

function removeWindow(index: number): void {
  windows.value = windows.value.filter((_, position) => position !== index)
}

async function save(): Promise<void> {
  pending.value = true
  error.value = null
  message.value = null
  const body: BandwidthScheduleRequest = {
    timezone: schedule.value.timezone,
    default_profile_id: schedule.value.default_profile_id ?? null,
    windows: windows.value.map(window => ({ ...window }))
  }
  const response = await api.PUT('/api/v1/bandwidth/schedule', { body })
  pending.value = false
  if (!response.data) return void (error.value = responseError(response))
  schedule.value = response.data
  message.value = t('bandwidth.schedule.saved')
  emit('changed')
}
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <FormListLayout :list-title="t('bandwidth.schedule.windows_title')" :count="windows.length">
      <template #form>
        <SectionHeader
          :eyebrow="t('bandwidth.schedule.eyebrow')"
          :title="t('bandwidth.schedule.title')"
          :description="t('bandwidth.schedule.description')"
          class="mb-4"
        />
        <UAlert v-if="error" class="mb-3" color="error" variant="subtle" :description="error" />
        <UAlert v-if="message" class="mb-3" color="success" variant="subtle" :description="message" />

        <div class="grid gap-3">
          <UFormField :label="t('bandwidth.schedule.timezone_label')" :description="t('bandwidth.schedule.timezone_description')">
            <TimezoneSelect v-model="schedule.timezone" :aria-label="t('bandwidth.schedule.timezone_label')" />
          </UFormField>
          <UFormField :label="t('bandwidth.schedule.default_label')" :description="t('bandwidth.schedule.default_description')">
            <USelect
              v-model="defaultProfileChoice"
              :items="[{ value: NO_SELECTION, label: t('bandwidth.schedule.no_default') }, ...profileItems]"
              value-key="value"
              class="w-full"
            />
          </UFormField>
          <div class="flex gap-2">
            <UButton type="button" icon="i-lucide-save" :label="t('common.actions.save')" :loading="pending" @click="save" />
          </div>
        </div>
      </template>
      <template #list-actions>
        <UButton type="button" size="xs" color="neutral" variant="outline" icon="i-lucide-plus" :disabled="!profiles.length" :label="t('bandwidth.schedule.add_window')" @click="addWindow" />
      </template>
      <template #list>
        <div class="space-y-3">
          <div v-for="(window, index) in windows" :key="index" class="border border-muted p-3">
            <div class="flex flex-wrap items-end gap-2">
              <USelect v-model="window.profile_id" :items="profileItems" value-key="value" class="w-44" :aria-label="t('bandwidth.schedule.window_profile')" />
              <UFormField :label="t('bandwidth.schedule.from')">
                <UInput :model-value="timeOf(window.start_minute)" type="time" class="w-28" @update:model-value="window.start_minute = minutesOf(String($event))" />
              </UFormField>
              <UFormField :label="t('bandwidth.schedule.to')">
                <UInput :model-value="timeOf(window.end_minute)" type="time" class="w-28" @update:model-value="window.end_minute = minutesOf(String($event))" />
              </UFormField>
              <UFormField :label="t('bandwidth.schedule.priority')">
                <UInput v-model.number="window.priority" type="number" class="w-24" />
              </UFormField>
              <USwitch v-model="window.enabled" :aria-label="t('bandwidth.schedule.enabled')" />
              <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.delete')" @click="removeWindow(index)" />
            </div>
            <div class="mt-2 flex flex-wrap gap-1">
              <UButton
                v-for="day in DAYS"
                :key="day"
                size="xs"
                :color="(window.days & (1 << day)) ? 'primary' : 'neutral'"
                :variant="(window.days & (1 << day)) ? 'solid' : 'outline'"
                :label="t(`bandwidth.days.${day}`)"
                @click="toggleDay(window, day)"
              />
            </div>
            <p v-if="window.end_minute <= window.start_minute" class="mt-2 text-xs text-muted">
              {{ t('bandwidth.schedule.wraps') }}
            </p>
          </div>
          <p v-if="!windows.length" class="border border-muted p-5 text-center text-sm text-muted">
            {{ profiles.length ? t('bandwidth.schedule.empty') : t('bandwidth.schedule.needs_profile') }}
          </p>
        </div>
      </template>
    </FormListLayout>
  </section>
</template>
