/**
 * RD-110-31: the poll interval is a setting, and this tab is where it is set.
 *
 * Three things pinned down: the field shows the value the settings document holds, saving it
 * writes the document back with only this field changed (read fresh, so an unsaved edit on
 * another tab is neither saved nor lost), and every folder row states the interval instead of
 * the "30 s" it used to print whatever the service did.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { HotFolder, Settings } from '@/api/types'
import routing from '@/locales/en/routing.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
const put = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: vi.fn(),
    PUT: (...args: unknown[]) => put(...args),
    PATCH: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: vi.fn(() => 'rejected'),
  resultMessage: vi.fn(() => '')
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))

const { default: RoutingHotfolders } = await import('./RoutingHotfolders.vue')

const FOLDER: HotFolder = {
  id: 'folder-1',
  name: 'Inbox',
  executor: { kind: 'daemon' },
  path: '/config/watch',
  recursive: false,
  category_id: null,
  import_mode: 'review',
  processed_path: 'processed',
  failed_path: 'failed',
  enabled: true
}

/** The document as the server holds it; the tab must not care about the rest of it. */
function stored(seconds: number): Settings {
  return { hotfolder_poll_seconds: seconds, stats_hourly_days: 30 } as Settings
}

function mount(seconds = 30) {
  return mountComponent(RoutingHotfolders, {
    messages: { routing },
    props: { modelValue: [FOLDER], settings: stored(seconds), categories: [] }
  })
}

function field(): HTMLInputElement {
  return screen.getByLabelText(routing.hotfolder.poll_label) as HTMLInputElement
}

describe('RoutingHotfolders poll interval', () => {
  beforeEach(() => {
    get.mockReset()
    put.mockReset()
  })

  it('shows the interval the settings document holds, on the field and on every row', () => {
    mount(45)

    expect(field().value).toBe('45')
    expect(screen.getByText(/45 s reconciliation/)).toBeTruthy()
  })

  it('saves the interval alone, into the document as the server holds it right now', async () => {
    // What the server holds differs from what this view loaded: another tab's unsaved edit
    // must not travel along, and a value saved elsewhere must not be overwritten.
    get.mockResolvedValue({ data: { ...stored(30), stats_hourly_days: 7 } })
    put.mockResolvedValue({ data: stored(120) })
    mount(30)

    await fireEvent.update(field(), '120')
    await fireEvent.click(screen.getByText(routing.hotfolder.poll_save))

    await waitFor(() => expect(put).toHaveBeenCalledTimes(1))
    expect(get).toHaveBeenCalledWith('/api/v1/settings')
    expect(put).toHaveBeenCalledWith('/api/v1/settings', {
      body: { hotfolder_poll_seconds: 120, stats_hourly_days: 7 }
    })
    await waitFor(() => expect(screen.getByText(routing.hotfolder.poll_saved)).toBeTruthy())
    expect(screen.getByText(/120 s reconciliation/)).toBeTruthy()
  })

  it('shows the refusal and keeps the rows on the value that is still in force', async () => {
    // A value the field's own bounds let through: below five the browser refuses the submit
    // itself, so the server's refusal has to be provoked with a value it dislikes for its own
    // reasons.
    get.mockResolvedValue({ data: stored(30) })
    put.mockResolvedValue({ error: { code: 'settings.hotfolder_poll_invalid' } })
    mount(30)

    await fireEvent.update(field(), '7')
    await fireEvent.click(screen.getByText(routing.hotfolder.poll_save))

    await waitFor(() => expect(screen.getByText('rejected')).toBeTruthy())
    expect(screen.getByText(/30 s reconciliation/)).toBeTruthy()
  })
})
