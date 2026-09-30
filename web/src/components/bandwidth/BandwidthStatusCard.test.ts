/**
 * The status card stops polling when it goes away, even in the middle of its first load.
 *
 * The 10-second interval used to be set after two awaits in `onMounted`; leaving the page while
 * those were in flight unmounted the card first, and the interval started afterwards with no
 * `onUnmounted` left to clear it.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import bandwidth from '@/locales/en/bandwidth.json'
import { mountComponent } from '@/test/mount'

const get = vi.hoisted(() => vi.fn())
vi.mock('@/api/client', () => ({
  api: { GET: get, POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: () => ''
}))

import BandwidthStatusCard from './BandwidthStatusCard.vue'

describe('the bandwidth status card', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    get.mockReset()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('does not keep polling after an unmount during its first load', async () => {
    let answer = (): void => {}
    get.mockReturnValueOnce(new Promise((resolve) => { answer = () => resolve({ data: undefined }) }))
    get.mockResolvedValue({ data: undefined })
    const { unmount } = mountComponent(BandwidthStatusCard, { messages: { bandwidth } })

    unmount()
    answer()
    await vi.advanceTimersByTimeAsync(0)
    const afterLoad = get.mock.calls.length
    await vi.advanceTimersByTimeAsync(60_000)

    expect(get.mock.calls.length).toBe(afterLoad)
  })

  it('polls the status while it is shown', async () => {
    get.mockResolvedValue({ data: undefined })
    const { unmount } = mountComponent(BandwidthStatusCard, { messages: { bandwidth } })
    await vi.advanceTimersByTimeAsync(0)
    const afterLoad = get.mock.calls.length

    await vi.advanceTimersByTimeAsync(20_000)

    expect(get.mock.calls.length).toBe(afterLoad + 2)
    unmount()
  })
})
