import type { Category } from '@/api/types'

/**
 * The categories a picker that assigns one by hand offers (RD-1260-03): those not hidden from the
 * selection, plus `current` — the category an entry already lies in, hidden or not — so the field
 * shows it instead of standing empty. Rules, subscriptions, hotfolders and the list filters keep
 * the whole list.
 */
export function pickableCategories(categories: Category[], current?: string | null): Category[] {
  return categories.filter(category => category.selectable !== false || category.id === current)
}
