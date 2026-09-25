/**
 * The statistics view (RD-110-01): the figures it draws come from the server's folded
 * buckets, the range buttons ask for another range, and an empty range says so instead of
 * drawing zeros as if they were data.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import stats from '@/locales/en/stats.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: vi.fn(),
    PUT: vi.fn(),
    PATCH: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: vi.fn(() => 'The service did not answer'),
  resultMessage: vi.fn(() => '')
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))

const { default: StatsView } = await import('./StatsView.vue')

const FIGURES = { completed: 0, failed: 0, retries: 0, bytes: 0, seconds: 0 }

const WEEK = {
  range: 'week',
  resolution: 'day',
  since: '2026-09-13T12:00:00Z',
  buckets: [
    { start: '2026-09-19T00:00:00Z', completed: 3, failed: 1, retries: 2, bytes: 3_221_225_472, seconds: 180 },
    { start: '2026-09-20T00:00:00Z', completed: 1, failed: 0, retries: 0, bytes: 1_073_741_824, seconds: 60 }
  ],
  by_kind: [{ key: 'http', completed: 4, failed: 1, retries: 2, bytes: 4_294_967_296, seconds: 240 }],
  by_provider: [
    { key: 'direct', completed: 3, failed: 1, retries: 2, bytes: 3_221_225_472, seconds: 180 },
    { key: 'rapidgator', completed: 1, failed: 0, retries: 0, bytes: 1_073_741_824, seconds: 60 }
  ],
  totals: { completed: 4, failed: 1, retries: 2, bytes: 4_294_967_296, seconds: 240 },
  all_time: { completed: 40, failed: 2, retries: 9, bytes: 42_949_672_960, seconds: 2_400 }
}

const EMPTY_DAY = {
  ...WEEK,
  range: 'day',
  resolution: 'hour',
  buckets: [],
  by_kind: [],
  by_provider: [],
  totals: { ...FIGURES }
}

function mount() {
  return mountComponent(StatsView, {
    messages: { stats },
    stubs: {
      UDashboardNavbar: { template: '<div><slot /><slot name="right" /></div>' }
    }
  })
}

describe('StatsView', () => {
  beforeEach(() => {
    get.mockReset()
    get.mockImplementation(async (_path: string, options?: unknown) => {
      const range = (options as { params: { query: { range: string } } }).params.query.range
      return { data: range === 'day' ? EMPTY_DAY : WEEK }
    })
  })

  it('says the range is empty rather than drawing zeros', async () => {
    mount()
    await waitFor(() => expect(get).toHaveBeenCalled())
    expect(await screen.findByText(stats.chart.empty)).toBeTruthy()
    expect(screen.queryByText(stats.groups.by_provider)).toBeNull()
  })

  it('asks for another range and draws its figures, groups and bars', async () => {
    mount()
    await waitFor(() => expect(get).toHaveBeenCalled())
    await fireEvent.click(await screen.findByRole('button', { name: stats.ranges.week }))
    await waitFor(() => expect(get).toHaveBeenLastCalledWith('/api/v1/stats/transfers', { params: { query: { range: 'week' } } }))

    const completed = await screen.findByText(stats.tiles.completed)
    expect(completed.nextElementSibling?.textContent).toBe('4')
    const bytes = screen.getByText(stats.tiles.bytes)
    expect(bytes.nextElementSibling?.textContent).toBe('4.0 GiB')
    const turnaround = screen.getByText(stats.tiles.turnaround)
    expect(turnaround.nextElementSibling?.textContent).toBe('1:00')
    expect(screen.getByText('rapidgator')).toBeTruthy()
    expect(screen.getByText(stats.groups.direct)).toBeTruthy()
    expect(screen.getByRole('img', { name: /3\.0 GiB/ })).toBeTruthy()
    expect(document.querySelectorAll('rect').length).toBe(2)
  })

  // Six local transfers that each finished inside their first second: a sum of 0 s, which the
  // tile used to draw as an empty value (RD-120-48).
  it('names a turnaround under a second instead of leaving the tile blank', async () => {
    const quick = { completed: 6, failed: 1, retries: 0, bytes: 144_703_488, seconds: 0 }
    get.mockImplementation(async () => ({
      data: { ...EMPTY_DAY, buckets: [{ start: '2026-09-23T21:00:00Z', ...quick }], totals: quick }
    }))
    mount()
    const turnaround = await screen.findByText(stats.tiles.turnaround)
    expect(turnaround.nextElementSibling?.textContent).toBe(stats.tiles.turnaround_under_second)
  })
})
