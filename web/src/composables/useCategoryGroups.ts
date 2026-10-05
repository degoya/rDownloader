import { computed, ref, watch, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Category, StorageRoot } from '@/api/types'
import { groupByRoot } from '@/utils/categoryGroups'

/**
 * The category list grouped by storage root, one accordion section per root that holds a category,
 * in the order of the roots (RD-150-13). A root without categories has no section — a control that
 * opens onto nothing is not rendered (`design.md`) — and while every category lies on one root
 * the list stays flat, because a single section would be a click that shows what was there.
 */
export function useCategoryGroups(categories: Ref<Category[]>, roots: () => StorageRoot[]) {
  const { t } = useI18n()
  const groups = computed(() => groupByRoot(categories.value, roots()))
  const grouped = computed(() => groups.value.length > 1)
  const sections = computed(() => groups.value.map(group => ({
    value: group.rootId,
    label: group.root?.name ?? t('routing.category.root_unknown'),
    path: group.root?.path ?? '',
    count: group.categories.length,
    hasDefault: group.categories.some(category => category.is_default),
    categories: group.categories
  })))
  /**
   * Every section starts open: the list reads as before, only with headings, and a reader closes
   * what is in the way. Decided without the running interface at hand (RD-150-13 leaves "all, or
   * only the default root with many categories" to a look at it); the section of the category
   * being edited, created or copied is opened whatever the reader had closed.
   */
  const openRoots = ref<string[]>([])
  const seenRoots = new Set<string>()
  watch(() => groups.value.map(group => group.rootId), (ids) => {
    const fresh = ids.filter(id => !seenRoots.has(id))
    fresh.forEach(id => seenRoots.add(id))
    if (fresh.length) openRoots.value = [...openRoots.value, ...fresh]
  }, { immediate: true })

  function openRootOf(category: Category): void {
    if (!openRoots.value.includes(category.storage_root_id)) {
      openRoots.value = [...openRoots.value, category.storage_root_id]
    }
  }

  return { grouped, sections, openRoots, openRootOf }
}
