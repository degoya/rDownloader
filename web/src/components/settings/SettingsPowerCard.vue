<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { PowerStatus, Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import WeekWindowRow from '@/components/WeekWindowRow.vue'
import { WHOLE } from '@/utils/numberInput'
import { EVERY_DAY, type WeekWindow } from '@/utils/weekWindows'
import SettingsCrossLink from '@/components/settings/SettingsCrossLink.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const status = ref<PowerStatus | null>(null)

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
function setWindows(windows: WeekWindow[], enabled = settings.value.quiet_hours?.enabled ?? false): void {
  settings.value.quiet_hours = { enabled, windows }
}

function addWindow(): void {
  setWindows(
    [...(settings.value.quiet_hours?.windows ?? []), { days: EVERY_DAY, start_minute: 23 * 60, end_minute: 7 * 60 }],
    settings.value.quiet_hours?.enabled ?? true
  )
}

function removeWindow(index: number): void {
  setWindows((settings.value.quiet_hours?.windows ?? []).filter((_, position) => position !== index))
}

function setWindow(index: number, window: WeekWindow): void {
  setWindows((settings.value.quiet_hours?.windows ?? []).map((entry, position) => (position === index ? window : entry)))
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
    <SettingsCrossLink class="mt-2" anchor="bandwidth.schedule" />

    <div v-if="settings.quiet_hours?.enabled" class="mt-3 space-y-3">
      <WeekWindowRow
        v-for="(window, index) in settings.quiet_hours.windows"
        :key="index"
        :model-value="window"
        @update:model-value="setWindow(index, $event)"
        @remove="removeWindow(index)"
      />
      <UButton type="button" color="neutral" variant="outline" size="xs" icon="i-lucide-plus" :label="t('power.quiet.add')" @click="addWindow" />
      <div class="grid gap-3">
        <UFormField :label="t('power.quiet.defer_postprocess')" orientation="horizontal">
          <USwitch v-model="settings.quiet_hours_defer_postprocess" />
        </UFormField>
        <UFormField :label="t('power.quiet.defer_notifications')" orientation="horizontal">
          <USwitch v-model="settings.quiet_hours_defer_notifications" />
        </UFormField>
        <SettingsCrossLink class="-mt-2" anchor="notifications.rules" />
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
      <SettingsCrossLink class="-mt-2" anchor="bandwidth.monthly" />
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
