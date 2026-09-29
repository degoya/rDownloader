import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it } from 'vitest'
import { effectScope, nextTick, ref } from 'vue'

import { usePublishedSelection, useSelectionStore } from './selection'

beforeEach(() => setActivePinia(createPinia()))

describe('useSelectionStore', () => {
  it('holds nothing for an empty selection', () => {
    const store = useSelectionStore()
    const owner = Symbol('view')
    store.publish(owner, { count: 2, bytes: 10n, unknown: 0 })
    expect(store.size).toEqual({ count: 2, bytes: 10n, unknown: 0 })
    store.publish(owner, { count: 0, bytes: 0n, unknown: 0 })
    expect(store.size).toBeNull()
  })

  it('lets only the view that published last clear the figure', () => {
    const store = useSelectionStore()
    const leaving = Symbol('leaving')
    const arriving = Symbol('arriving')
    store.publish(leaving, { count: 1, bytes: 1n, unknown: 0 })
    store.publish(arriving, { count: 3, bytes: 30n, unknown: 1 })
    store.release(leaving)
    expect(store.size).toEqual({ count: 3, bytes: 30n, unknown: 1 })
    store.release(arriving)
    expect(store.size).toBeNull()
  })

  it('follows a view while it is mounted and clears when it goes', async () => {
    const store = useSelectionStore()
    const size = ref({ count: 1, bytes: 5n, unknown: 0 })
    const scope = effectScope()
    scope.run(() => usePublishedSelection(size))
    expect(store.size?.bytes).toBe(5n)
    size.value = { count: 2, bytes: 9n, unknown: 0 }
    await nextTick()
    expect(store.size?.bytes).toBe(9n)
    scope.stop()
    expect(store.size).toBeNull()
  })
})
