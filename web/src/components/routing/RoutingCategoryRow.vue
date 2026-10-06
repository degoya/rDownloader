<script setup lang="ts">
/**
 * One category in the list beside the category form.
 *
 * Its own component since the list is drawn two ways — flat, and grouped by storage root in an
 * accordion (RD-150-13) — and the row is the same in both.
 */
import { useI18n } from 'vue-i18n'

import type { CollisionPolicy } from '@/api/storage'
import type { Category } from '@/api/types'
import { editingRowClass } from '@/utils/editingRow'

const props = defineProps<{
  category: Category
  /** Where the category's folder lies, "<root> / <path>". */
  location: string
  editing: boolean
  deleting: boolean
  duplicating: boolean
  /** The category's own collision policy (RD-150-01); absent when it inherits the global one. */
  collision?: CollisionPolicy | undefined
}>()
const emit = defineEmits<{ edit: [], duplicate: [], remove: [] }>()
const { t } = useI18n()

function cleanupSummary(category: Category): string {
  const extensions = category.cleanup_extensions
  if (!extensions) return t('routing.category.cleanup_inherit_badge')
  if (!extensions.length) return t('routing.category.cleanup_none_badge')
  return t('routing.category.cleanup_list_badge', { count: extensions.length })
}
</script>

<template>
  <div class="p-3" :class="editingRowClass(props.editing)" data-category-row>
    <div class="flex items-center gap-2">
      <span class="size-2" :style="{ backgroundColor: props.category.color }" />
      <p class="min-w-0 flex-1 truncate text-sm font-medium text-highlighted">{{ props.category.name }}</p>
      <UBadge v-if="props.editing" size="sm" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
      <UBadge v-if="props.category.is_default" size="sm" color="primary" variant="subtle">{{ t('routing.category.default_badge') }}</UBadge>
      <UButton
        size="xs"
        color="neutral"
        variant="ghost"
        icon="i-lucide-copy-plus"
        :label="t('common.actions.duplicate')"
        :title="t('common.duplicate_hint')"
        :loading="props.duplicating"
        @click="emit('duplicate')"
      />
      <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('common.actions.edit')" :title="t('common.actions.edit')" @click="emit('edit')" />
      <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.delete')" :title="t('common.actions.delete')" :loading="props.deleting" @click="emit('remove')" />
    </div>
    <p class="mt-1 truncate font-mono text-2xs text-muted">{{ props.location }}</p>
    <div class="mt-2 flex flex-wrap items-center gap-1">
      <UBadge size="sm" color="neutral" variant="subtle">{{ t('routing.category.level_badge', { level: props.category.postprocess_level ?? t('routing.category.inherit_short') }) }}</UBadge>
      <UBadge size="sm" color="neutral" variant="subtle" class="font-mono">{{ t('routing.category.script_badge', { script: props.category.script ?? t('routing.category.inherit_short') }) }}</UBadge>
      <UBadge size="sm" color="neutral" variant="subtle">{{ cleanupSummary(props.category) }}</UBadge>
      <UBadge v-if="props.collision" size="sm" color="neutral" variant="subtle" data-testid="category-collision-badge">
        {{ t('routing.category.collision_badge', { policy: t(`downloads.collision.policies.${props.collision}`) }) }}
      </UBadge>
    </div>
  </div>
</template>
