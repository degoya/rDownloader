<script setup lang="ts">
/**
 * One row of a profile list beside its form (RD-1280-11): the avatar, the name with a mono line
 * under it, the badges, and test, edit and delete. The authentication profiles, the object
 * storage profiles and the remote credentials drew it three times.
 */
import { useI18n } from 'vue-i18n'

import { editingRowClass } from '@/utils/editingRow'

const props = withDefaults(defineProps<{
  icon: string
  name: string
  /** The mono line under the name; the `subtitle` slot replaces it. */
  subtitle?: string
  editing: boolean
  /** How the row being edited is marked (`utils/editingRow.ts`). */
  editingShape?: 'stripe' | 'outline'
  /** A request for this row is running: test and delete show it. */
  busy?: boolean
  /** The edit button's name, `common.actions.edit` when unset. */
  editLabel?: string
}>(), { subtitle: '', editingShape: 'outline', busy: false, editLabel: '' })

const emit = defineEmits<{ test: [], edit: [], remove: [] }>()

const { t } = useI18n()
</script>

<template>
  <div class="flex flex-wrap items-center gap-3 p-3" :class="editingRowClass(props.editing, props.editingShape)">
    <UAvatar :icon="props.icon" color="primary" />
    <div class="min-w-0 flex-1">
      <p class="text-sm font-medium text-highlighted">{{ props.name }}</p>
      <p class="truncate font-mono text-2xs text-muted">
        <slot name="subtitle">{{ props.subtitle }}</slot>
      </p>
    </div>
    <UBadge v-if="props.editing" size="sm" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
    <slot name="badges" />
    <slot name="controls" />
    <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-plug-zap" :label="t('common.actions.test')" :loading="props.busy" @click="emit('test')" />
    <UButton
      size="xs"
      color="neutral"
      variant="ghost"
      icon="i-lucide-pencil"
      :aria-label="props.editLabel || t('common.actions.edit')"
      :title="props.editLabel || t('common.actions.edit')"
      @click="emit('edit')"
    />
    <UButton
      size="xs"
      color="error"
      variant="ghost"
      icon="i-lucide-trash-2"
      :aria-label="t('common.actions.delete')"
      :title="t('common.actions.delete')"
      :loading="props.busy"
      @click="emit('remove')"
    />
  </div>
</template>
