<script setup lang="ts">
/**
 * The trigger of the automation editor (`AutomationView.vue`), with the schedule a time trigger
 * runs on (RD-1240-10): an interval in minutes counted from midnight, or a cron line, both read in
 * the service's time zone.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AutomationSchedule } from '@/api/types'
import { type AutomationTrigger, MAX_INTERVAL_MINUTES } from '@/composables/useAutomationDraft'

const props = defineProps<{
  triggerOptions: { value: AutomationTrigger, label: string }[]
  /** A package action is left over from the trigger before; a time trigger cannot run it. */
  packageActionOnSchedule: boolean
}>()
const trigger = defineModel<AutomationTrigger>('trigger', { required: true })
const schedule = defineModel<AutomationSchedule>('schedule', { required: true })
const { t } = useI18n()

const kindOptions = computed(() => [
  { value: 'interval', label: t('automation.schedule.interval') },
  { value: 'cron', label: t('automation.schedule.cron') }
])

function changeKind(kind: string): void {
  if (kind === schedule.value.kind) return
  schedule.value = kind === 'cron' ? { kind: 'cron', expression: '0 6 * * *' } : { kind: 'interval', minutes: 60 }
}
</script>

<template>
  <div class="grid gap-4 border border-muted p-3" data-testid="automation-trigger-card">
    <!-- The trigger is the automation's kind, so it comes first, before its name. -->
    <UFormField :label="t('automation.trigger_label')" :description="t('automation.trigger_help')">
      <USelectMenu
        :model-value="trigger"
        :items="props.triggerOptions"
        value-key="value"
        class="w-full"
        @update:model-value="(value: AutomationTrigger) => (trigger = value)"
      />
    </UFormField>
    <template v-if="trigger === 'schedule'">
      <UFormField :label="t('automation.schedule.heading')" :description="t('automation.schedule.help')">
        <USelect
          :model-value="schedule.kind"
          :items="kindOptions"
          :aria-label="t('automation.schedule.heading')"
          class="w-56"
          @update:model-value="changeKind"
        />
      </UFormField>
      <UFormField
        v-if="schedule.kind === 'interval'"
        :label="t('automation.schedule.minutes')"
        :description="t('automation.schedule.interval_help')"
      >
        <UInputNumber
          :model-value="schedule.minutes"
          :min="1"
          :max="MAX_INTERVAL_MINUTES"
          class="w-40"
          @update:model-value="(minutes: number | null) => (schedule = { kind: 'interval', minutes: minutes ?? 1 })"
        />
      </UFormField>
      <UFormField v-else :label="t('automation.schedule.expression')" :description="t('automation.schedule.cron_help')">
        <UInput
          :model-value="schedule.expression"
          maxlength="120"
          class="w-64 font-mono"
          placeholder="0 6 * * *"
          @update:model-value="(expression: string) => (schedule = { kind: 'cron', expression })"
        />
      </UFormField>
      <p v-if="props.packageActionOnSchedule" class="text-xs text-error">{{ t('automation.schedule.no_package') }}</p>
    </template>
  </div>
</template>
