/**
 * The status card stops polling when it goes away, even in the middle of its first load.
 *
 * The 10-second interval used to be set after two awaits in `onMounted`; leaving the page while
 * those were in flight unmounted the card first, and the interval started afterwards with no
 * `onUnmounted` left to clear it.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import bandwidth from '@/locales/en/bandwidth.json'
import { mountComponent } from '@/test/mount'

const get = vi.hoisted(() => vi.fn())
const put = vi.hoisted(() => vi.fn())
const remove = vi.hoisted(() => vi.fn())
vi.mock('@/api/client', () => ({
  api: { GET: get, POST: vi.fn(), PUT: put, DELETE: remove },
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

/** A profile switched on by hand (RD-190-20): why the active one is active, switching, and back. */
describe('the bandwidth status card switching a profile by hand', () => {
  const PROFILES = [
    { id: 'p-day', name: 'Day', scopes: [] },
    { id: 'p-night', name: 'Night', scopes: [] }
  ]
  const STATUS = { timezone: 'UTC', budget_exhausted: false, source: 'schedule', active_profile: PROFILES[0] }

  beforeEach(() => {
    vi.useRealTimers()
    get.mockReset()
    put.mockReset()
    remove.mockReset()
    get.mockImplementation(async (path: string) =>
      ({ data: path === '/api/v1/bandwidth/status' ? STATUS : [] }))
  })

  async function mounted() {
    const view = mountComponent(BandwidthStatusCard, { props: { profiles: PROFILES }, messages: { bandwidth } })
    await screen.findByTestId('bandwidth-source')
    return view
  }

  it('says the schedule chose the active profile', async () => {
    await mounted()
    expect(screen.getByTestId('bandwidth-source').textContent?.trim()).toBe('Chosen by the schedule')
    expect(screen.queryByRole('button', { name: 'Back to the schedule' })).toBeNull()
  })

  it('switches to the chosen profile until the chosen end, and says so', async () => {
    const until = '2026-10-02T15:00:00Z'
    put.mockResolvedValue({
      data: { ...STATUS, source: 'manual', active_profile: PROFILES[1], manual: { profile_id: 'p-night', ends: 'at', until, switched_at: until } }
    })
    await mounted()
    const [profile, ends] = screen.getAllByRole('combobox')
    await fireEvent.update(profile as HTMLElement, 'p-night')
    await fireEvent.update(ends as HTMLElement, 'never')

    await fireEvent.click(screen.getByRole('button', { name: 'Switch' }))

    expect(put).toHaveBeenCalledWith('/api/v1/bandwidth/manual', { body: { profile_id: 'p-night', ends: 'never' } })
    await waitFor(() => expect(screen.getByTestId('bandwidth-source').textContent).toMatch(/^\s*Switched by hand, until /))
    expect(screen.getByRole('button', { name: 'Back to the schedule' })).toBeTruthy()
  })

  it('asks for an hour as a time from now, and no limits without a profile', async () => {
    put.mockResolvedValue({ data: STATUS })
    await mounted()
    const [profile, ends] = screen.getAllByRole('combobox')
    await fireEvent.update(profile as HTMLElement, 'none')
    await fireEvent.update(ends as HTMLElement, '60')
    const before = Date.now()

    await fireEvent.click(screen.getByRole('button', { name: 'Switch' }))

    const body = put.mock.calls[0]?.[1]?.body as { profile_id: null, ends: string, until: string }
    expect(body.profile_id).toBeNull()
    expect(body.ends).toBe('at')
    expect(Date.parse(body.until) - before).toBeGreaterThanOrEqual(60 * 60_000 - 1_000)
    expect(Date.parse(body.until) - before).toBeLessThanOrEqual(60 * 60_000 + 1_000)
  })

  it('hands back to the schedule', async () => {
    get.mockImplementation(async (path: string) => ({
      data: path === '/api/v1/bandwidth/status'
        ? { ...STATUS, source: 'manual', manual: { profile_id: 'p-day', ends: 'never', until: null, switched_at: '2026-10-02T12:00:00Z' } }
        : []
    }))
    remove.mockResolvedValue({ data: STATUS })
    await mounted()
    expect(screen.getByTestId('bandwidth-source').textContent?.trim()).toBe('Switched by hand, until you switch back')

    await fireEvent.click(screen.getByRole('button', { name: 'Back to the schedule' }))

    expect(remove).toHaveBeenCalledWith('/api/v1/bandwidth/manual')
    await waitFor(() => expect(screen.getByTestId('bandwidth-source').textContent?.trim()).toBe('Chosen by the schedule'))
  })
})
