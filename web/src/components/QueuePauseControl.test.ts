/**
 * The global pause control with its timed pause (RD-190-20): the durations it offers, what a
 * choice asks the server for, and the paused state that resumes at a click.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import downloads from '@/locales/en/downloads.json'
import { useQueuePauseStore } from '@/stores/queuePause'
import { useTransfersStore } from '@/stores/transfers'
import { mountComponent } from '@/test/mount'

import QueuePauseControl from './QueuePauseControl.vue'

vi.mock('@/api/client', () => ({
  api: {
    GET: vi.fn(async () => ({ data: undefined })),
    POST: vi.fn(async () => ({ data: undefined })),
    PUT: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: vi.fn(() => 'refused'),
  errorMessage: vi.fn(),
  resultMessage: vi.fn()
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: vi.fn(() => () => {}) }))

const NOW = new Date('2026-10-02T12:00:00Z')

function mount(placement: 'header' | 'rail' = 'header') {
  mountComponent(QueuePauseControl, { props: { placement }, messages: { downloads } })
  return { queuePause: useQueuePauseStore(), transfers: useTransfersStore() }
}

describe('QueuePauseControl', () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(NOW)
    vi.mocked(api.PUT).mockReset()
    vi.mocked(api.DELETE).mockReset()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('offers three durations and a time of the person’s choosing', () => {
    mount()
    for (const label of ['Pause for 30 minutes', 'Pause for 1 hour', 'Pause for 3 hours', 'Pause until…']) {
      expect(screen.getByRole('button', { name: label })).toBeTruthy()
    }
    expect(screen.getByRole('button', { name: 'Pause for a while' })).toBeTruthy()
  })

  it('pauses for the chosen duration and says until when', async () => {
    const until = new Date(NOW.getTime() + 60 * 60_000).toISOString()
    vi.mocked(api.PUT).mockResolvedValue({ data: { paused: true, until, files: 3 } } as never)
    const { queuePause, transfers } = mount()

    await fireEvent.click(screen.getByRole('button', { name: 'Pause for 1 hour' }))
    await vi.waitFor(() => expect(transfers.notice).toMatch(/^Queue paused until /))

    expect(queuePause.active).toBe(true)
    expect(vi.mocked(api.PUT)).toHaveBeenCalledWith('/api/v1/queue/pause', { body: { minutes: 60 } })
    expect(screen.getByTestId('queue-pause-resume').textContent).toMatch(/^Paused until /)
  })

  it('resumes everything at a click while a timed pause holds', async () => {
    const until = new Date(NOW.getTime() + 30 * 60_000).toISOString()
    vi.mocked(api.PUT).mockResolvedValue({ data: { paused: true, until, files: 2 } } as never)
    vi.mocked(api.DELETE).mockResolvedValue({ data: { resumed: 2 } } as never)
    const { queuePause, transfers } = mount('rail')
    await queuePause.pauseFor(30)
    await vi.waitFor(() => expect(screen.queryByTestId('queue-pause-resume')).toBeTruthy())

    await fireEvent.click(screen.getByRole('button', { name: 'Resume now' }))
    await vi.waitFor(() => expect(transfers.notice).toBe('2 transfers were started or resumed.'))

    expect(queuePause.active).toBe(false)
    expect(vi.mocked(api.DELETE)).toHaveBeenCalledWith('/api/v1/queue/pause')
    expect(screen.queryByTestId('queue-pause-resume')).toBeNull()
  })

  it('shows a refusal where the list shows its errors', async () => {
    vi.mocked(api.PUT).mockResolvedValue({ data: undefined } as never)
    const { transfers } = mount()

    await fireEvent.click(screen.getByRole('button', { name: 'Pause for 30 minutes' }))

    await vi.waitFor(() => expect(transfers.error).toBe('refused'))
  })
})

/**
 * RD-1190-14: an account whose traffic is used up holds downloads back without anybody pausing
 * them; the control says whose and when it is checked next, and starting by hand lets it go.
 */
describe('QueuePauseControl account traffic', () => {
  const hold = {
    account_id: 'account-1', account_label: 'DDownload premium', provider: 'ddownload', action: 'pause_queue',
    until: '2026-10-02T13:00:00Z', next_check_at: '2026-10-02T12:15:00Z'
  }

  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(NOW)
    vi.mocked(api.DELETE).mockReset()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('names the account and lets a start by hand go past the hold', async () => {
    const { queuePause } = mount()
    queuePause.accountTraffic = [hold] as never
    await Promise.resolve()

    const notice = screen.getByTestId('account-traffic-notice')
    expect(notice.innerHTML).toContain('DDownload premium')
    vi.mocked(api.DELETE).mockResolvedValue({ data: { resumed: 0 } } as never)

    await fireEvent.click(screen.getByTestId('account-traffic-continue'))

    expect(vi.mocked(api.DELETE)).toHaveBeenCalledWith('/api/v1/queue/pause')
    expect(queuePause.accountTraffic).toEqual([])
  })

  it('shows nothing while no account is held', () => {
    mount()
    expect(screen.queryByTestId('account-traffic-notice')).toBeNull()
  })
})
