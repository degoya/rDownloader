/**
 * RD-110-31: the poll interval is a setting, and this tab is where it is set.
 *
 * Three things pinned down: the field shows the value the settings document holds, saving it
 * writes the document back with only this field changed (read fresh, so an unsaved edit on
 * another tab is neither saved nor lost), and every folder row states the interval instead of
 * the "30 s" it used to print whatever the service did.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { HotFolder, Settings } from '@/api/types'
import common from '@/locales/en/common.json'
import routing from '@/locales/en/routing.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
const put = vi.fn()
const post = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
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

/**
 * RD-150-12: two hot folders may not watch one path, so a copy is not stored first: the form
 * takes the original's settings under a free name and asks for another folder before creating.
 */
describe('RoutingHotfolders duplicate', () => {
  beforeEach(() => {
    post.mockReset()
  })

  it('fills the form with a copy that is created only once its folder changed', async () => {
    const original: HotFolder = { ...FOLDER, recursive: true, import_mode: 'enqueue', category_id: null, processed_path: 'done', failed_path: 'broken' }
    mountComponent(RoutingHotfolders, {
      messages: { routing },
      props: { modelValue: [original], settings: stored(30), categories: [] },
      // The shared stub drops the description, and the description is what says the path must change.
      stubs: {
        UFormField: {
          props: ['label', 'description'],
          template: '<div><label v-if="label">{{ label }}<slot /></label><slot v-else /><p v-if="description">{{ description }}</p></div>'
        }
      }
    })

    await fireEvent.click(within(screen.getByText('Inbox').closest('div.border') as HTMLElement).getByRole('button', { name: common.actions.duplicate }))

    expect(post).not.toHaveBeenCalled()
    expect(screen.getByRole('heading', { level: 3, name: routing.hotfolder.form_new })).toBeTruthy()
    expect((screen.getByPlaceholderText(routing.hotfolder.name_placeholder) as HTMLInputElement).value).toBe('Inbox (copy)')
    expect(screen.getByText(routing.hotfolder.copy_path_hint)).toBeTruthy()
    // A draft, not an edit: the button still creates, and the cross drops the draft.
    expect(screen.getByRole('button', { name: routing.hotfolder.create })).toBeTruthy()
    expect(screen.getByRole('button', { name: common.actions.cancel_edit })).toBeTruthy()

    post.mockResolvedValue({ data: { ...original, id: 'folder-2', name: 'Inbox (copy)', path: '/config/watch-2' } })
    await fireEvent.update(screen.getByPlaceholderText(routing.hotfolder.path_placeholder), '/config/watch-2')
    await fireEvent.click(screen.getByRole('button', { name: routing.hotfolder.create }))

    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    expect(post.mock.calls[0]?.[1]?.body).toMatchObject({
      name: 'Inbox (copy)',
      path: '/config/watch-2',
      recursive: true,
      import_mode: 'enqueue',
      processed_path: 'done',
      failed_path: 'broken'
    })
    await waitFor(() => expect(screen.queryByText(routing.hotfolder.copy_path_hint)).toBeNull())
  })
})

/** RD-1101-10: the dot beside a folder is colour only, so the switched-off state is a word too. */
describe('RoutingHotfolders state', () => {
  it('names a disabled folder in text and keeps the colour dot out of the accessibility tree', () => {
    mountComponent(RoutingHotfolders, {
      messages: { routing },
      props: { modelValue: [FOLDER, { ...FOLDER, id: 'folder-2', name: 'Paused', enabled: false }], settings: stored(30), categories: [] }
    })

    const paused = screen.getByText('Paused').closest('div.border') as HTMLElement
    expect(within(paused).getByText(routing.hotfolder.disabled_badge)).toBeTruthy()
    const active = screen.getByText('Inbox').closest('div.border') as HTMLElement
    expect(within(active).queryByText(routing.hotfolder.disabled_badge)).toBeNull()
    expect(paused.querySelector('[data-chip-dot]')).toBeNull()
    expect(active.querySelector('[data-chip-dot]')).not.toBeNull()
    for (const dot of document.querySelectorAll('[data-chip]')) expect(dot.textContent).toBe('')
  })
})
