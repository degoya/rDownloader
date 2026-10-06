<script setup lang="ts">
/** One automation in the list beside the automation editor (`AutomationView.vue`), with its last runs. */
import { useI18n } from 'vue-i18n'

import type { Automation, AutomationRun } from '@/api/types'
import { formatMoment } from '@/utils/format'
import { editingRowClass } from '@/utils/editingRow'

const props = defineProps<{
  automation: Automation
  editing: boolean
  duplicating: boolean
  /** The automation's runs, newest first. */
  runs: AutomationRun[]
}>()
const emit = defineEmits<{ toggle: [enabled: boolean], duplicate: [], versions: [], edit: [], remove: [] }>()
const { t } = useI18n()
</script>

<template>
  <article class="p-4" :class="editingRowClass(props.editing, 'stripe')">
    <div class="flex flex-wrap items-center gap-3">
      <UChip standalone color="success" :show="props.automation.enabled" class="w-2" />
      <div class="min-w-0 flex-1">
        <p class="truncate text-sm font-medium text-highlighted">{{ props.automation.name }}</p>
        <p class="text-xs text-muted">
          {{ t(`automation.trigger.${props.automation.definition?.trigger ?? 'download_completed'}`) }}
          · {{ t('automation.version', { version: props.automation.version }) }}
          · {{ t('automation.action_count', { count: props.automation.definition?.actions?.length ?? 0 }) }}
        </p>
      </div>
      <UBadge v-if="props.editing" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
      <USwitch
        :model-value="props.automation.enabled"
        :aria-label="t('automation.enabled')"
        @update:model-value="(value: boolean) => emit('toggle', value)"
      />
      <UButton
        icon="i-lucide-copy-plus"
        size="xs"
        color="neutral"
        variant="ghost"
        :label="t('common.actions.duplicate')"
        :title="t('automation.duplicate_hint')"
        :loading="props.duplicating"
        @click="emit('duplicate')"
      />
      <UButton
        icon="i-lucide-history"
        size="xs"
        color="neutral"
        variant="ghost"
        :aria-label="t('automation.history.open')"
        :title="t('automation.history.open')"
        @click="emit('versions')"
      />
      <UButton
        icon="i-lucide-pencil"
        size="xs"
        color="neutral"
        variant="ghost"
        :aria-label="t('common.actions.edit')"
        :title="t('common.actions.edit')"
        @click="emit('edit')"
      />
      <UButton
        icon="i-lucide-trash-2"
        size="xs"
        color="error"
        variant="ghost"
        :aria-label="t('automation.remove.confirm')"
        :title="t('automation.remove.confirm')"
        @click="emit('remove')"
      />
    </div>
    <ul v-if="props.runs.length" class="mt-3 space-y-1">
      <li
        v-for="run in props.runs.slice(0, 5)"
        :key="run.id"
        class="flex flex-wrap items-center gap-2 text-xs text-muted"
      >
        <span class="font-medium">{{ t(`automation.run_state.${run.state}`) }}</span>
        <span class="numeric">{{ formatMoment(run.started_at) }}</span>
        <span v-if="run.message" class="truncate">{{ run.message }}</span>
      </li>
    </ul>
    <p v-else class="mt-3 text-xs text-muted">{{ t('automation.no_runs') }}</p>
  </article>
</template>
