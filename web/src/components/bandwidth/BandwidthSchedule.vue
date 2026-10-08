<script setup lang="ts">
import { computed, nextTick, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { BandwidthProfile, BandwidthSchedule, BandwidthScheduleRequest } from '@/api/types'
import FormActions from '@/components/FormActions.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import TimezoneSelect from '@/components/TimezoneSelect.vue'
import WeekWindowRow from '@/components/WeekWindowRow.vue'
import { PLAIN, isNumber } from '@/utils/numberInput'
import { NO_SELECTION, optionalSelection, selectionValue } from '@/utils/select'
import { EVERY_DAY } from '@/utils/weekWindows'
import FormFeedback from '@/components/FormFeedback.vue'
import SearchableSelect from '@/components/SearchableSelect.vue'
import SettingsCrossLink from '@/components/settings/SettingsCrossLink.vue'

const props = defineProps<{ profiles: BandwidthProfile[] }>()
const schedule = defineModel<BandwidthSchedule>({ required: true })
const emit = defineEmits<{ changed: [] }>()
const { t } = useI18n()
const pending = ref(false)
const error = ref<string | null>(null)
const message = ref<string | null>(null)
const windowList = ref<HTMLElement | null>(null)

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

function addWindow(): void {
  const first = props.profiles[0]
  if (!first) return
  windows.value = [
    ...windows.value,
    { profile_id: first.id, days: EVERY_DAY, start_minute: 22 * 60, end_minute: 6 * 60, priority: 0, enabled: true }
  ]
}

/**
 * Copies a window directly below its original (RD-150-12). A window has no name and no route of
 * its own — the schedule is stored whole — so the copy is edited in place like every window, and
 * the focus moves to its first control; it is kept once the schedule is saved.
 */
async function duplicateWindow(index: number): Promise<void> {
  const source = windows.value[index]
  if (!source) return
  windows.value = [...windows.value.slice(0, index + 1), { ...source }, ...windows.value.slice(index + 1)]
  await nextTick()
  windowList.value
    ?.querySelectorAll<HTMLElement>('[data-window]')[index + 1]
    ?.querySelector<HTMLElement>('input, select, [role="combobox"]')
    ?.focus()
}

function removeWindow(index: number): void {
  windows.value = windows.value.filter((_, position) => position !== index)
}

async function save(): Promise<void> {
  error.value = null
  message.value = null
  // The windows sit outside the form, so their emptied priority is held here (RD-1110-10).
  if (windows.value.some(window => !isNumber(window.priority))) return void (error.value = t('settings.messages.number_empty'))
  pending.value = true
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
  <UCard as="section" data-settings-anchor="bandwidth.schedule">
    <FormListLayout :list-title="t('bandwidth.schedule.windows_title')" :count="windows.length">
      <template #form>
        <SectionHeader
          :eyebrow="t('bandwidth.schedule.eyebrow')"
          :title="t('bandwidth.schedule.title')"
          :description="t('bandwidth.schedule.description')"
          class="mb-2"
        />
        <SettingsCrossLink class="mb-4" anchor="unattended.quiet_hours" />
        <FormFeedback class="mb-3" :error="error" :message="message" />

        <form class="grid gap-3" @submit.prevent="save">
          <UFormField :label="t('bandwidth.schedule.timezone_label')" :description="t('bandwidth.schedule.timezone_description')">
            <TimezoneSelect v-model="schedule.timezone" :aria-label="t('bandwidth.schedule.timezone_label')" />
          </UFormField>
          <UFormField :label="t('bandwidth.schedule.default_label')" :description="t('bandwidth.schedule.default_description')">
            <SearchableSelect
              v-model="defaultProfileChoice"
              :items="[{ value: NO_SELECTION, label: t('bandwidth.schedule.no_default') }, ...profileItems]"
              class="w-full"
            />
          </UFormField>
          <FormActions :create-label="t('common.actions.save')" create-icon="i-lucide-save" :loading="pending" />
        </form>
      </template>
      <template #list-actions>
        <UButton type="button" size="xs" color="neutral" variant="outline" icon="i-lucide-plus" :disabled="!profiles.length" :label="t('bandwidth.schedule.add_window')" @click="addWindow" />
      </template>
      <template #list>
        <div ref="windowList" class="space-y-3">
          <WeekWindowRow v-for="(window, index) in windows" :key="index" data-window :model-value="window" @update:model-value="windows[index] = $event" @remove="removeWindow(index)">
            <template #leading>
              <SearchableSelect v-model="window.profile_id" :items="profileItems" class="w-44" :aria-label="t('bandwidth.schedule.window_profile')" />
            </template>
            <template #actions>
              <UFormField :label="t('bandwidth.schedule.priority')">
                <UInputNumber v-model="window.priority" required :format-options="PLAIN" class="w-24" />
              </UFormField>
              <USwitch v-model="window.enabled" :aria-label="t('bandwidth.schedule.enabled')" />
              <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-copy-plus" :label="t('common.actions.duplicate')" :title="t('bandwidth.schedule.duplicate_hint')" @click="duplicateWindow(index)" />
            </template>
            <p v-if="window.end_minute <= window.start_minute" class="mt-2 text-xs text-muted">
              {{ t('bandwidth.schedule.wraps') }}
            </p>
          </WeekWindowRow>
          <p v-if="!windows.length" class="border border-muted p-5 text-center text-sm text-muted">
            {{ profiles.length ? t('bandwidth.schedule.empty') : t('bandwidth.schedule.needs_profile') }}
          </p>
        </div>
      </template>
    </FormListLayout>
  </UCard>
</template>
