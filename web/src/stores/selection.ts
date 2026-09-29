import { defineStore } from 'pinia'
import { onScopeDispose, ref, watch, type Ref } from 'vue'

import type { SelectionSize } from '@/utils/selectionSize'

/**
 * The selection the active view publishes for the status bar.
 *
 * The selection itself stays in the view; this holds only its summary, and only while the view
 * that set it is mounted. The owner token keeps a view that unmounts after the next one mounted
 * from clearing its successor's figure.
 */
export const useSelectionStore = defineStore('selection', () => {
  const size = ref<SelectionSize | null>(null)
  let owner: symbol | null = null

  function publish(by: symbol, value: SelectionSize): void {
    owner = by
    size.value = value.count ? value : null
  }

  function release(by: symbol): void {
    if (owner !== by) return
    owner = null
    size.value = null
  }

  return { size, publish, release }
})

/** Publishes a view's selection size for as long as the calling component is mounted. */
export function usePublishedSelection(size: Ref<SelectionSize>): void {
  const store = useSelectionStore()
  const token = Symbol('selection')
  watch(size, value => store.publish(token, value), { immediate: true })
  onScopeDispose(() => store.release(token))
}
