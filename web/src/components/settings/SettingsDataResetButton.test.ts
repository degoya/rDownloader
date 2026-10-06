import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

import SettingsDataResetButton from './SettingsDataResetButton.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'This clears data for good and must be confirmed.')
}))

/** The clear asks first; the tests drive the answer and read the question. */
type ConfirmOptions = { title: string, description: string, confirmIcon?: string, destructive?: boolean }
const confirmed = vi.fn(async (_options: ConfirmOptions) => true)
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirmed }))

const added = vi.fn()
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: added }) }))

type Target = 'logs' | 'audit' | 'stats' | 'notifications' | 'notifications_pending' | 'storage_operations' | 'content_index' | 'history'

function renderButton(props: { target: Target, count: number | null }) {
  return mountComponent(SettingsDataResetButton, { messages: { system }, props })
}

beforeEach(() => {
  vi.clearAllMocks()
  confirmed.mockResolvedValue(true)
  vi.mocked(api.POST).mockResolvedValue({ data: { removed: 41 } } as never)
})

describe('the question', () => {
  // The whole point of the confirmation: a number somebody reads, not a habit they click past.
  it('names how many records will go before anything is cleared', async () => {
    renderButton({ target: 'logs', count: 41 })

    await fireEvent.click(screen.getByRole('button'))

    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    const options = confirmed.mock.calls[0]?.[0] as ConfirmOptions
    expect(options.description).toContain('41')
    expect(options.title).toBe('Clear the service log?')
    expect(options.destructive).toBe(true)
    expect(api.POST).toHaveBeenCalled()
  })

  it('says that the audit clear writes itself into the emptied log', async () => {
    renderButton({ target: 'audit', count: 9 })

    await fireEvent.click(screen.getByRole('button'))

    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    const options = confirmed.mock.calls[0]?.[0] as ConfirmOptions
    expect(options.description).toContain('first entry')
  })

  // A delivery still owed an attempt survives the clear (RD-130-08); the question says so, or
  // the person reads a list that is not empty afterwards as a clear that failed.
  it('says that queued and retrying notifications stay', async () => {
    renderButton({ target: 'notifications', count: 12 })

    await fireEvent.click(screen.getByRole('button'))

    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    const options = confirmed.mock.calls[0]?.[0] as ConfirmOptions
    expect(options.title).toBe('Clear the notification history?')
    expect(options.description).toContain('12')
    expect(options.description).toContain('queued or retrying')
  })

  // The second button at the history (RD-170-11) cancels notifications; its question has to
  // say that they will not be sent, or it reads like the clear next to it.
  it('says that discarded notifications will never be sent', async () => {
    renderButton({ target: 'notifications_pending', count: 3 })

    expect(screen.getByRole('button').textContent).toBe('Discard pending')
    await fireEvent.click(screen.getByRole('button'))

    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    const options = confirmed.mock.calls[0]?.[0] as ConfirmOptions & { confirmLabel?: string }
    expect(options.title).toBe('Discard the pending notifications?')
    expect(options.description).toContain('3')
    expect(options.description).toContain('never sent')
    expect(options.confirmLabel).toBe('Discard for good')
    expect(options.destructive).toBe(true)
  })

  // The storage history keeps a running operation (RD-180-13); the question says so, and that
  // no file moves.
  it('says that a running operation stays when the storage history is cleared', async () => {
    renderButton({ target: 'storage_operations', count: 7 })

    expect(screen.getByRole('button').textContent).toBe('Clear history')
    await fireEvent.click(screen.getByRole('button'))

    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    const options = confirmed.mock.calls[0]?.[0] as ConfirmOptions
    expect(options.title).toBe('Clear the storage history?')
    expect(options.description).toContain('7')
    expect(options.description).toContain('still running stays')
    expect(options.description).toContain('No file is moved or deleted')
  })

  // What clearing the index costs, and what brings it back (RD-180-13): without it the person
  // cannot tell a clear from losing duplicate detection for good.
  it('names what an emptied content index no longer recognises and how it comes back', async () => {
    renderButton({ target: 'content_index', count: 40 })

    expect(screen.getByRole('button').textContent).toBe('Clear index')
    await fireEvent.click(screen.getByRole('button'))

    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    const options = confirmed.mock.calls[0]?.[0] as ConfirmOptions & { confirmLabel?: string }
    expect(options.title).toBe('Clear the content index?')
    expect(options.description).toContain('40')
    expect(options.description).toContain('not recognised as a content duplicate')
    expect(options.description).toContain('Check the content index')
    expect(options.description).toContain('downloaded again')
    expect(options.confirmLabel).toBe('Clear the index')
    expect(options.destructive).toBe(true)
  })

  it('clears nothing when the question is answered with no', async () => {
    confirmed.mockResolvedValue(false)
    renderButton({ target: 'stats', count: 3 })

    await fireEvent.click(screen.getByRole('button'))

    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    expect(api.POST).not.toHaveBeenCalled()
  })
})

describe('the request', () => {
  it('carries the confirmation as a value, not only as a dialog', async () => {
    renderButton({ target: 'logs', count: 41 })

    await fireEvent.click(screen.getByRole('button'))

    await waitFor(() => expect(api.POST).toHaveBeenCalled())
    const [path, options] = vi.mocked(api.POST).mock.calls[0] as unknown as [string, { body: { confirmed: boolean } }]
    expect(path).toBe('/api/v1/diagnostics/logs/clear')
    expect(options.body).toEqual({ confirmed: true })
  })

  it.each([
    ['audit', '/api/v1/audit/records/clear'],
    ['stats', '/api/v1/stats/transfers/clear'],
    ['notifications', '/api/v1/notifications/deliveries/clear'],
    ['notifications_pending', '/api/v1/notifications/deliveries/discard-pending'],
    ['storage_operations', '/api/v1/storage/operations/clear'],
    ['content_index', '/api/v1/storage/content-index/clear'],
    ['history', '/api/v1/history/clear']
  ] as const)('%s posts to its own route', async (target, path) => {
    renderButton({ target, count: 2 })

    await fireEvent.click(screen.getByRole('button'))

    await waitFor(() => expect(api.POST).toHaveBeenCalled())
    expect(vi.mocked(api.POST).mock.calls[0]?.[0]).toBe(path)
  })
})

describe('what it shows', () => {
  it('shows how much is stored', () => {
    renderButton({ target: 'logs', count: 41 })
    expect(screen.getByTestId('data-reset-logs-count').textContent).toContain('41')
  })

  it('reports what a storage clear removed in its own words', async () => {
    renderButton({ target: 'content_index', count: 41 })

    await fireEvent.click(screen.getByRole('button'))

    await waitFor(() => expect(added).toHaveBeenCalled())
    expect((added.mock.calls[0]?.[0] as { title: string }).title).toBe('41 index entries removed')
  })

  it('reports the number of records removed', async () => {
    renderButton({ target: 'logs', count: 41 })

    await fireEvent.click(screen.getByRole('button'))

    await waitFor(() => expect(added).toHaveBeenCalled())
    expect((added.mock.calls[0]?.[0] as { title: string }).title).toContain('41')
  })

  it('shows the refusal instead of claiming success', async () => {
    vi.mocked(api.POST).mockResolvedValue({ error: { code: 'data_reset.not_confirmed' } } as never)
    renderButton({ target: 'logs', count: 41 })

    await fireEvent.click(screen.getByRole('button'))

    expect(await screen.findByTestId('data-reset-logs-error')).toBeTruthy()
    expect(added).not.toHaveBeenCalled()
  })

  // A control that can only report "0 records removed" reads as a broken feature.
  it('is dead while the store is already empty', () => {
    renderButton({ target: 'stats', count: 0 })
    expect((screen.getByRole('button') as HTMLButtonElement).disabled).toBe(true)
  })

  it('says nothing about a count it does not have yet', () => {
    renderButton({ target: 'stats', count: null })
    expect(screen.queryByTestId('data-reset-stats-count')).toBeNull()
  })
})
