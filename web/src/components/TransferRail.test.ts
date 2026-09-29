/**
 * The selection figure in the status bar (RD-170-14): shown while a list has something ticked,
 * gone when it has not, and a lower bound where a size is still unknown.
 */
import { screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: undefined })), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(),
  errorMessage: vi.fn()
}))

import downloads from '@/locales/en/downloads.json'
import { useSelectionStore } from '@/stores/selection'
import { mountComponent } from '@/test/mount'

import TransferRail from './TransferRail.vue'

function mount() {
  mountComponent(TransferRail, { messages: { downloads }, stubs: { SpeedHistoryChart: true } })
  return useSelectionStore()
}

describe('TransferRail selection', () => {
  it('is hidden while nothing is selected', () => {
    mount()
    expect(screen.queryByTestId('rail-selection')).toBeNull()
  })

  it('shows the count and the summed size, and goes when the selection empties', async () => {
    const store = mount()
    const owner = Symbol('view')
    store.publish(owner, { count: 3, bytes: 5n * 1024n ** 3n, unknown: 0 })
    await nextTick()
    const figure = screen.getByTestId('rail-selection')
    expect(figure.textContent?.replace(/\s+/g, ' ').trim()).toBe('3 selected · 5.0 GiB')
    expect(figure.getAttribute('title')).toBe('3 selected, 5.0 GiB in total')

    store.release(owner)
    await nextTick()
    expect(screen.queryByTestId('rail-selection')).toBeNull()
  })

  it('marks a sum that leaves out unknown sizes as a lower bound', async () => {
    const store = mount()
    store.publish(Symbol('view'), { count: 4, bytes: 2048n, unknown: 2 })
    await nextTick()
    const figure = screen.getByTestId('rail-selection')
    expect(figure.textContent).toContain('≥ 2.0 KiB')
    expect(figure.getAttribute('title')).toBe('4 selected, at least 2.0 KiB in total – the size of 2 is not known yet')
  })

  it('shows only the count when no size is known at all', async () => {
    const store = mount()
    store.publish(Symbol('view'), { count: 2, bytes: 0n, unknown: 2 })
    await nextTick()
    const figure = screen.getByTestId('rail-selection')
    expect(figure.textContent).not.toContain('·')
    expect(figure.getAttribute('title')).toBe('2 selected, size not known yet')
  })
})
