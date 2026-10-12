<script setup lang="ts">
/**
 * The level the service logs at, in the settings document (RD-1260-12).
 *
 * A field of the page like any other: saved with the page, applied by the service from the next
 * line without a restart. The Log view has the same choice as a shortcut and the debug window
 * beside it. When the service was started with `RUST_LOG`, that decides instead: the choice is
 * locked and the notice names the value.
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { fetchLogLevel, SERVICE_LOG_LEVELS, type LogLevelSettings, type ServiceLogLevel } from '@/api/logLevel'
import type { Settings } from '@/api/types'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const rustLog = ref<string | null>(null)

const items = computed(() => SERVICE_LOG_LEVELS.map(level => ({ value: level, label: t(`logs.levels.${level}`) })))

const level = computed<ServiceLogLevel>({
  get: () => (settings.value as LogLevelSettings).log_level ?? 'info',
  set: (value) => {
    (settings.value as LogLevelSettings).log_level = value
  }
})

onMounted(async () => {
  const answer = await fetchLogLevel()
  if (answer.ok) rustLog.value = answer.data.rust_log
})
</script>

<template>
  <div class="grid gap-3" data-testid="settings-log-level">
    <UFormField :label="t('settings.logs.level_label')" :description="t('settings.logs.level_description')">
      <USelect
        v-model="level"
        :items="items"
        value-key="value"
        :disabled="rustLog !== null"
        class="mt-2 w-full"
        data-testid="settings-log-level-select"
      />
    </UFormField>
    <UAlert
      v-if="rustLog !== null"
      color="warning"
      variant="subtle"
      icon="i-lucide-lock"
      :description="t('logs.recording.locked', { value: rustLog })"
      data-testid="settings-log-level-locked"
    />
  </div>
</template>
