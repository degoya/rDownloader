/**
 * The jump to one row of the download list (RD-1240-14): which keys it accepts, that it waits
 * for the queue, opens a file's package, lifts a filter that hides the row and forgets the
 * request once done.
 */
import { describe, expect, it, vi } from 'vitest'
import { computed, effectScope, nextTick, reactive, ref } from 'vue'

import { revealKey, useQueueReveal } from './useQueueReveal'

const route = reactive({ path: '/downloads', hash: '', query: {} as Record<string, string> })
const replace = vi.fn(async (location: { query: Record<string, string> }) => { route.query = location.query })
vi.mock('vue-router', () => ({ useRoute: () => route, useRouter: () => ({ replace }) }))

function flush(): Promise<void> {
  return new Promise(resolve => setTimeout(resolve))
}

describe('revealKey', () => {
  it('takes a package or a file key and nothing else', () => {
    expect(revealKey('package:0190a1b2-0000-7000-8000-000000000001')).toBe('package:0190a1b2-0000-7000-8000-000000000001')
    expect(revealKey('file:abc')).toBe('file:abc')
    expect(revealKey('candidate:abc')).toBeNull()
    expect(revealKey('file:a"b')).toBeNull()
    expect(revealKey(['file:abc'])).toBeNull()
    expect(revealKey(undefined)).toBeNull()
  })
})

describe('useQueueReveal', () => {
  it('waits for the queue, opens the package, lifts the filter, focuses the row and forgets', async () => {
    route.query = { reveal: 'file:d1', filter: 'failed' }
    replace.mockClear()
    const settled = ref(false)
    const open = ref<string[]>([])
    const filtered = ref(true)
    const keys = computed(() => {
      const shown = ['package:p1', ...(open.value.includes('p1') ? ['file:d1'] : [])]
      return filtered.value ? [] : shown
    })
    const focusRow = vi.fn(async () => true)
    const scope = effectScope()
    scope.run(() => useQueueReveal({
      settled: () => settled.value,
      packageOfFile: id => id === 'd1' ? 'p1' : undefined,
      rowKeys: keys,
      openPackage: id => { open.value = [...open.value, id] },
      filterActive: computed(() => filtered.value),
      resetFilter: () => { filtered.value = false },
      list: ref({ focusRow })
    }))
    await flush()
    expect(focusRow).not.toHaveBeenCalled()

    settled.value = true
    await nextTick()
    await flush()
    expect(open.value).toEqual(['p1'])
    expect(filtered.value).toBe(false)
    expect(focusRow).toHaveBeenCalledWith('file:d1')
    expect(route.query).toEqual({ filter: 'failed' })
    scope.stop()
  })

  it('forgets a row the queue no longer holds without focusing anything', async () => {
    route.query = { reveal: 'package:gone' }
    const focusRow = vi.fn(async () => true)
    const scope = effectScope()
    scope.run(() => useQueueReveal({
      settled: () => true,
      packageOfFile: () => undefined,
      rowKeys: computed(() => ['package:p1']),
      openPackage: vi.fn(),
      filterActive: computed(() => false),
      resetFilter: vi.fn(),
      list: ref({ focusRow })
    }))
    await flush()
    expect(focusRow).not.toHaveBeenCalled()
    expect(route.query).toEqual({})
    scope.stop()
  })
})
