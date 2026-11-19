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
  <!--
    The row measures its own width (`@container`, RD-1430-03), as an account row does: in a
    phone's column *Duplicate* keeps only its icon, the badges wrap under the name instead of
    pushing the actions out, and the name is cut, never an action. `min-w-0` lets the row
    shrink to its list's width — without it the list grew to the row's content, 531 px in a
    358 px card.
  -->
  <div class="@container min-w-0 p-3" :class="editingRowClass(props.editing)" data-category-row>
    <div class="flex items-center gap-2">
      <span class="size-2 shrink-0" :style="{ backgroundColor: props.category.color }" />
      <div class="flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-1" data-category-title>
        <p class="min-w-0 grow truncate text-sm font-medium text-highlighted">{{ props.category.name }}</p>
        <UBadge v-if="props.editing" size="sm" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
        <UBadge v-if="props.category.is_default" size="sm" color="primary" variant="subtle">{{ t('routing.category.default_badge') }}</UBadge>
        <UBadge v-if="props.category.selectable === false" size="sm" color="neutral" variant="outline" icon="i-lucide-eye-off" :title="t('routing.category.hidden_badge_title')" data-testid="category-hidden-badge">{{ t('routing.category.hidden_badge') }}</UBadge>
      </div>
      <div class="flex shrink-0 items-center gap-2" data-category-actions>
        <UButton
          size="xs"
          color="neutral"
          variant="ghost"
          icon="i-lucide-copy-plus"
          :label="t('common.actions.duplicate')"
          :aria-label="t('common.actions.duplicate')"
          :title="t('common.duplicate_hint')"
          :ui="{ label: '@max-sm:hidden' }"
          :loading="props.duplicating"
          @click="emit('duplicate')"
        />
        <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('common.actions.edit')" :title="t('common.actions.edit')" @click="emit('edit')" />
        <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.delete')" :title="t('common.actions.delete')" :loading="props.deleting" @click="emit('remove')" />
      </div>
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
