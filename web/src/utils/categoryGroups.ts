import type { Category, StorageRoot } from '@/api/types'

interface CategoryGroup {
  rootId: string
  /** The storage root, or `undefined` for categories whose root the list does not know. */
  root: StorageRoot | undefined
  categories: Category[]
}

/**
 * Groups categories by their storage root, in the order of the roots (RD-150-13).
 *
 * A root without categories has no group. Categories pointing at a root the list does not hold
 * — a root deleted elsewhere, or a list that has not arrived yet — are gathered at the end
 * rather than dropped, so no category disappears from the list because of grouping.
 */
export function groupByRoot(categories: Category[], roots: StorageRoot[]): CategoryGroup[] {
  const known = roots
    .map(root => ({ rootId: root.id, root, categories: categories.filter(category => category.storage_root_id === root.id) }))
    .filter(group => group.categories.length > 0)
  const rootIds = new Set(roots.map(root => root.id))
  const orphans = new Map<string, Category[]>()
  for (const category of categories) {
    if (rootIds.has(category.storage_root_id)) continue
    orphans.set(category.storage_root_id, [...(orphans.get(category.storage_root_id) ?? []), category])
  }
  return [
    ...known,
    ...[...orphans].map(([rootId, members]) => ({ rootId, root: undefined, categories: members }))
  ]
}
