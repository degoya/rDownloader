import { defineStore, storeToRefs } from 'pinia'

import { api } from '@/api/client'
import type { Category } from '@/api/types'

import { sharedRead } from './sharedRead'

/** The categories, read once for every view that lists or picks one (WEB-3). */
export const useCategoriesStore = defineStore('categories', () => {
  const shared = sharedRead(() => api.GET('/api/v1/categories'), [] as Category[], ['category.changed'])
  return { categories: shared.value, fetchCategories: shared.load, follow: shared.follow }
})

/**
 * The categories for a component: the shared list, kept current while the component lives, and
 * `fetchCategories()` for the read it makes when it opens.
 */
export function useCategories() {
  const store = useCategoriesStore()
  store.follow()
  return { categories: storeToRefs(store).categories, fetchCategories: store.fetchCategories }
}
