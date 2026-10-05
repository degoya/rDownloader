<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { PowerStatus, Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import { WHOLE } from '@/utils/numberInput'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const status = ref<PowerStatus | null>(null)

/** Monday-first, matching the backend's bitmask. */
const DAYS = [0, 1, 2, 3, 4, 5, 6]
const dayItems = computed(() => DAYS.map(day => ({ value: day, label: t(`bandwidth.days.${day}`) })))

const actions = computed(() =>
  (['none', 'script', 'standby', 'shutdown'] as const).map(value => ({
    value,
    label: t(`power.action.${value}`),
    disabled:
      (value === 'standby' && status.value?.capabilities.standby === false) ||
      (value === 'shutdown' && status.value?.capabilities.shutdown === false)
  }))
)

const destructive = computed(() =>
  settings.value.completion_action === 'standby' || settings.value.completion_action === 'shutdown'
)

/** The window list is edited in place; the settings document is saved by the tab. */
function addWindow(): void {
  settings.value.quiet_hours = {
    enabled: settings.value.quiet_hours?.enabled ?? true,
    windows: [
      ...(settings.value.quiet_hours?.windows ?? []),
      { days: 0b0111_1111, start_minute: 23 * 60, end_minute: 7 * 60 }
    ]
  }
}

function removeWindow(index: number): void {
  settings.value.quiet_hours = {
    enabled: settings.value.quiet_hours?.enabled ?? false,
    windows: (settings.value.quiet_hours?.windows ?? []).filter((_, position) => position !== index)
  }
}

/** The window's bitmask as the list of days a checkbox group holds. */
function daysOf(mask: number): number[] {
  return DAYS.filter(day => (mask & (1 << day)) !== 0)
}

function setDays(index: number, days: number[]): void {
  const windows = [...(settings.value.quiet_hours?.windows ?? [])]
  const window = windows[index]
  if (!window) return
  windows[index] = { ...window, days: days.reduce((mask, day) => mask | (1 << day), 0) }
  settings.value.quiet_hours = { enabled: settings.value.quiet_hours?.enabled ?? false, windows }
}

function timeOf(minutes: number): string {
  const hours = Math.floor(minutes / 60)
  return `${String(hours).padStart(2, '0')}:${String(minutes % 60).padStart(2, '0')}`
}

function setTime(index: number, key: 'start_minute' | 'end_minute', value: string): void {
  const [hours, minutes] = value.split(':').map(Number)
  const total = Math.min(Math.max((hours ?? 0) * 60 + (minutes ?? 0), 0), 1440)
  const windows = [...(settings.value.quiet_hours?.windows ?? [])]
  const window = windows[index]
  if (!window) return
  windows[index] = { ...window, [key]: total }
  settings.value.quiet_hours = { enabled: settings.value.quiet_hours?.enabled ?? false, windows }
}

onMounted(async () => {
  const response = await api.GET('/api/v1/power/status')
  if (response.data) status.value = response.data
})
</script>

<template>
  <UCard as="section" data-settings-anchor="unattended.power">
    <SectionHeader :eyebrow="t('power.card.eyebrow')" :title="t('power.card.title')" :description="t('power.card.description')" class="mb-4" />

    <USeparator class="mb-4" />
    <UFormField data-settings-anchor="unattended.quiet_hours" :label="t('power.quiet.label')" :description="t('power.quiet.description')" orientation="horizontal">
      <USwitch
        :model-value="settings.quiet_hours?.enabled ?? false"
        @update:model-value="settings.quiet_hours = { enabled: Boolean($event), windows: settings.quiet_hours?.windows ?? [] }"
      />
    </UFormField>

    <div v-if="settings.quiet_hours?.enabled" class="mt-3 space-y-3">
      <div v-for="(window, index) in settings.quiet_hours.windows" :key="index" class="border border-muted p-3">
        <div class="flex flex-wrap items-end gap-2">
          <UFormField :label="t('power.quiet.from')">
            <UInput :model-value="timeOf(window.start_minute)" type="time" class="w-28" @update:model-value="setTime(index, 'start_minute', String($event))" />
          </UFormField>
          <UFormField :label="t('power.quiet.to')">
            <UInput :model-value="timeOf(window.end_minute)" type="time" class="w-28" @update:model-value="setTime(index, 'end_minute', String($event))" />
          </UFormField>
          <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.delete')" :title="t('common.actions.delete')" @click="removeWindow(index)" />
        </div>
        <UCheckboxGroup
          class="mt-2"
          :model-value="daysOf(window.days)"
          :items="dayItems"
          :legend="t('bandwidth.schedule.days_label')"
          orientation="horizontal"
          size="sm"
          @update:model-value="(days: number[]) => setDays(index, days)"
        />
      </div>
      <UButton type="button" color="neutral" variant="outline" size="xs" icon="i-lucide-plus" :label="t('power.quiet.add')" @click="addWindow" />
      <div class="grid gap-3">
        <UFormField :label="t('power.quiet.defer_postprocess')" orientation="horizontal">
          <USwitch v-model="settings.quiet_hours_defer_postprocess" />
        </UFormField>
        <UFormField :label="t('power.quiet.defer_notifications')" orientation="horizontal">
          <USwitch v-model="settings.quiet_hours_defer_notifications" />
        </UFormField>
      </div>
    </div>

    <USeparator class="my-4" />
    <div class="grid gap-3">
      <UFormField data-settings-anchor="unattended.completion" :label="t('power.completion.label')" :description="t('power.completion.description')">
        <USelect v-model="settings.completion_action" :items="actions" value-key="value" class="w-full" />
      </UFormField>
      <UFormField v-if="settings.completion_action === 'script'" :label="t('power.completion.script_label')" :description="t('power.completion.script_description')">
        <UInput v-model="settings.completion_script" class="w-full font-mono" placeholder="on-idle.sh" icon="i-lucide-scroll-text" />
      </UFormField>
      <UFormField v-if="destructive" hint="s" :label="t('power.completion.countdown_label')" :description="t('power.completion.countdown_description')">
        <UInputNumber v-model="settings.completion_countdown_seconds" required :min="10" :max="3600" :format-options="WHOLE" class="w-full" />
      </UFormField>
      <template v-if="destructive">
        <UFormField :label="t('power.completion.approval_label')" :description="t('power.completion.approval_description')" orientation="horizontal">
          <USwitch v-model="settings.power_actions_allowed" />
        </UFormField>
        <p v-if="!settings.power_actions_allowed" class="-mt-2 text-xs leading-5 text-warning">{{ t('power.completion.approval_missing') }}</p>
      </template>
    </div>

    <USeparator class="my-4" />
    <div class="grid gap-3">
      <UFormField
        :label="t('power.context.battery_label')"
        :description="status && !status.capabilities.battery ? t('power.context.unavailable') : undefined"
        orientation="horizontal"
      >
        <USwitch v-model="settings.pause_on_battery" :disabled="status?.capabilities.battery === false" />
      </UFormField>
      <UFormField
        :label="t('power.context.metered_label')"
        :description="status && !status.capabilities.metered ? t('power.context.unavailable') : undefined"
        orientation="horizontal"
      >
        <USwitch v-model="settings.pause_on_metered" :disabled="status?.capabilities.metered === false" />
      </UFormField>
      <UFormField
        data-settings-anchor="unattended.prevent_standby"
        :label="t('power.context.prevent_standby_label')"
        :description="status && !status.capabilities.inhibit_standby ? t('power.context.unavailable') : t('power.context.prevent_standby_description')"
        orientation="horizontal"
      >
        <USwitch v-model="settings.prevent_standby" :disabled="status?.capabilities.inhibit_standby === false" />
      </UFormField>
      <UFormField :label="t('power.context.prevent_display_label')" :description="t('power.context.prevent_display_description')" orientation="horizontal">
        <USwitch v-model="settings.prevent_display_standby" :disabled="!settings.prevent_standby || status?.capabilities.inhibit_display === false" />
      </UFormField>
    </div>
    <p v-if="status?.inhibiting" class="mt-3 flex items-center gap-1.5 text-xs text-muted">
      <UIcon name="i-lucide-coffee" class="size-3.5 shrink-0" />
      <span>{{ t('power.context.inhibiting') }}</span>
    </p>
  </UCard>
</template>
