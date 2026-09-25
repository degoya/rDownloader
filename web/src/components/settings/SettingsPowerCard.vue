<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { PowerStatus, Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const status = ref<PowerStatus | null>(null)

/** Monday-first, matching the backend's bitmask. */
const DAYS = [0, 1, 2, 3, 4, 5, 6]

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

function toggleDay(index: number, day: number): void {
  const windows = [...(settings.value.quiet_hours?.windows ?? [])]
  const window = windows[index]
  if (!window) return
  windows[index] = { ...window, days: window.days ^ (1 << day) }
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
  <section class="border border-muted bg-default p-5">
    <SectionHeader :eyebrow="t('power.card.eyebrow')" :title="t('power.card.title')" :description="t('power.card.description')" class="mb-4" />

    <div class="flex items-center justify-between gap-5 border-t border-muted pt-4">
      <div>
        <p class="text-sm font-medium text-highlighted">{{ t('power.quiet.label') }}</p>
        <p class="mt-1 text-xs leading-5 text-muted">{{ t('power.quiet.description') }}</p>
      </div>
      <USwitch
        :model-value="settings.quiet_hours?.enabled ?? false"
        :aria-label="t('power.quiet.label')"
        @update:model-value="settings.quiet_hours = { enabled: Boolean($event), windows: settings.quiet_hours?.windows ?? [] }"
      />
    </div>

    <div v-if="settings.quiet_hours?.enabled" class="mt-3 space-y-3">
      <div v-for="(window, index) in settings.quiet_hours.windows" :key="index" class="border border-muted p-3">
        <div class="flex flex-wrap items-end gap-2">
          <UFormField :label="t('power.quiet.from')">
            <UInput :model-value="timeOf(window.start_minute)" type="time" class="w-28" @update:model-value="setTime(index, 'start_minute', String($event))" />
          </UFormField>
          <UFormField :label="t('power.quiet.to')">
            <UInput :model-value="timeOf(window.end_minute)" type="time" class="w-28" @update:model-value="setTime(index, 'end_minute', String($event))" />
          </UFormField>
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
            @click="toggleDay(index, day)"
          />
        </div>
      </div>
      <UButton type="button" color="neutral" variant="outline" size="xs" icon="i-lucide-plus" :label="t('power.quiet.add')" @click="addWindow" />
      <div class="grid gap-3">
        <div class="flex items-center justify-between gap-3">
          <p class="text-xs leading-5 text-muted">{{ t('power.quiet.defer_postprocess') }}</p>
          <USwitch v-model="settings.quiet_hours_defer_postprocess" :aria-label="t('power.quiet.defer_postprocess')" />
        </div>
        <div class="flex items-center justify-between gap-3">
          <p class="text-xs leading-5 text-muted">{{ t('power.quiet.defer_notifications') }}</p>
          <USwitch v-model="settings.quiet_hours_defer_notifications" :aria-label="t('power.quiet.defer_notifications')" />
        </div>
      </div>
    </div>

    <div class="mt-4 grid gap-3 border-t border-muted pt-4">
      <UFormField :label="t('power.completion.label')" :description="t('power.completion.description')">
        <USelect v-model="settings.completion_action" :items="actions" value-key="value" class="w-full" />
      </UFormField>
      <UFormField v-if="settings.completion_action === 'script'" :label="t('power.completion.script_label')" :description="t('power.completion.script_description')">
        <UInput v-model="settings.completion_script" class="w-full font-mono" placeholder="on-idle.sh" icon="i-lucide-scroll-text" />
      </UFormField>
      <UFormField v-if="destructive" :label="t('power.completion.countdown_label')" :description="t('power.completion.countdown_description')">
        <UInput v-model.number="settings.completion_countdown_seconds" type="number" min="10" max="3600" class="w-full" icon="i-lucide-timer">
          <template #trailing><span class="font-mono text-xs text-muted">s</span></template>
        </UInput>
      </UFormField>
      <div v-if="destructive" class="flex items-center justify-between gap-5">
        <div>
          <p class="text-sm font-medium text-highlighted">{{ t('power.completion.approval_label') }}</p>
          <p class="mt-1 text-xs leading-5 text-muted">{{ t('power.completion.approval_description') }}</p>
          <p v-if="!settings.power_actions_allowed" class="mt-1 text-xs leading-5 text-warning">{{ t('power.completion.approval_missing') }}</p>
        </div>
        <USwitch v-model="settings.power_actions_allowed" :aria-label="t('power.completion.approval_label')" />
      </div>
    </div>

    <div class="mt-4 grid gap-3 border-t border-muted pt-4">
      <div class="flex items-center justify-between gap-3">
        <div>
          <p class="text-sm font-medium text-highlighted">{{ t('power.context.battery_label') }}</p>
          <p v-if="status && !status.capabilities.battery" class="mt-1 text-xs leading-5 text-muted">{{ t('power.context.unavailable') }}</p>
        </div>
        <USwitch v-model="settings.pause_on_battery" :disabled="status?.capabilities.battery === false" :aria-label="t('power.context.battery_label')" />
      </div>
      <div class="flex items-center justify-between gap-3">
        <div>
          <p class="text-sm font-medium text-highlighted">{{ t('power.context.metered_label') }}</p>
          <p v-if="status && !status.capabilities.metered" class="mt-1 text-xs leading-5 text-muted">{{ t('power.context.unavailable') }}</p>
        </div>
        <USwitch v-model="settings.pause_on_metered" :disabled="status?.capabilities.metered === false" :aria-label="t('power.context.metered_label')" />
      </div>
      <div class="flex items-center justify-between gap-3">
        <div>
          <p class="text-sm font-medium text-highlighted">{{ t('power.context.prevent_standby_label') }}</p>
          <p class="mt-1 text-xs leading-5 text-muted">
            {{ status && !status.capabilities.inhibit_standby ? t('power.context.unavailable') : t('power.context.prevent_standby_description') }}
          </p>
        </div>
        <USwitch v-model="settings.prevent_standby" :disabled="status?.capabilities.inhibit_standby === false" :aria-label="t('power.context.prevent_standby_label')" />
      </div>
      <div class="flex items-center justify-between gap-3">
        <div>
          <p class="text-sm font-medium text-highlighted">{{ t('power.context.prevent_display_label') }}</p>
          <p class="mt-1 text-xs leading-5 text-muted">{{ t('power.context.prevent_display_description') }}</p>
        </div>
        <USwitch v-model="settings.prevent_display_standby" :disabled="!settings.prevent_standby || status?.capabilities.inhibit_display === false" :aria-label="t('power.context.prevent_display_label')" />
      </div>
    </div>
    <p v-if="status?.inhibiting" class="mt-3 flex items-center gap-1.5 text-xs text-muted">
      <UIcon name="i-lucide-coffee" class="size-3.5 shrink-0" />
      <span>{{ t('power.context.inhibiting') }}</span>
    </p>
  </section>
</template>
