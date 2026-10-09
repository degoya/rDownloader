/**
 * The download list without the card above it (RD-1220-03): the direct job opens as a dialog from
 * the navbar or with `a`, and the queue's notices go as toasts. `DownloadsView.test.ts` holds the
 * list itself; this file holds what stands around it.
 */
import { fireEvent, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'

import { api } from '@/api/client'
import { openDirectAdd } from '@/composables/directAddAction'
import { SHORTCUT_DEFINITIONS } from '@/composables/shortcutDefinitions'
import downloads from '@/locales/en/downloads.json'
import torrent from '@/locales/en/torrent.json'
import { useTransfersStore } from '@/stores/transfers'
import { mountComponent } from '@/test/mount'

import DownloadsView from './DownloadsView.vue'

vi.mock('@/api/client', () => ({
  api: {
    GET: vi.fn(async () => ({ data: [] })),
    POST: vi.fn(async () => ({ data: undefined })),
    PUT: vi.fn(async () => ({ data: undefined })),
    PATCH: vi.fn(async () => ({ data: undefined })),
    DELETE: vi.fn(async () => ({ data: undefined }))
  },
  responseError: vi.fn(() => 'The address is not supported'),
  errorMessage: vi.fn(),
  resultMessage: vi.fn()
}))
const toasts = vi.hoisted(() => ({ added: [] as Record<string, unknown>[] }))
vi.mock('@nuxt/ui/composables', () => ({
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(false) }) }) }),
  useToast: () => ({ add: (toast: Record<string, unknown>) => toasts.added.push(toast) })
}))
vi.mock('vue-router', async importOriginal => ({
  ...await importOriginal<typeof import('vue-router')>(),
  useRoute: () => ({ path: '/downloads', hash: '', query: {} }),
  useRouter: () => ({ replace: vi.fn(async () => undefined), push: vi.fn() })
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))

/** A dialog that is in the page only while it is open, as the real one's content is. */
const UModal = {
  props: ['open', 'title', 'description'],
  emits: ['update:open'],
  template: '<div v-if="open" role="dialog" :aria-label="title"><slot name="body" /><slot name="footer" /></div>'
}
const SearchableSelect = { props: ['modelValue', 'items'], template: '<select v-bind="$attrs"></select>' }
const UKbd = { props: ['value'], template: '<kbd>{{ value }}</kbd>' }
const UButton = {
  props: ['label', 'disabled', 'loading', 'ariaLabel'],
  template: '<button type="button" v-bind="$attrs" :disabled="disabled" :aria-label="ariaLabel">{{ label }}<slot /><slot name="trailing" /></button>'
}
const UDashboardNavbar = { template: '<header><slot name="right" /></header>' }
// What surrounds the list and is not what these cases are about.
const quiet = Object.fromEntries([
  'QueuePauseControl', 'QueueResetFailedMenu', 'PowerCountdownAlert', 'StorageCapacityAlert', 'TorrentKillSwitchAlert',
  'CollisionPromptsAlert', 'PostprocessQueue', 'QueueSummary', 'DataState'
].map(name => [name, true]))

function mountView() {
  return mountComponent(DownloadsView, { messages: { downloads, torrent }, stubs: { ...quiet, UModal, UButton, UKbd, UDashboardNavbar, SearchableSelect } })
}

const pressA = SHORTCUT_DEFINITIONS.find(definition => definition.keys === 'a')!.handler

beforeEach(() => {
  toasts.added = []
  vi.mocked(api.POST).mockReset()
})

describe('the direct job', () => {
  it('opens from the navbar button, which carries its key and the tour mark', async () => {
    const { getByTestId, queryByRole, getByRole } = mountView()
    const button = getByTestId('downloads-add')
    expect(button.getAttribute('data-tour')).toBe('downloads-add')
    expect(button.querySelector('kbd')?.textContent).toBe('a')
    expect(queryByRole('dialog')).toBeNull()

    await fireEvent.click(button)
    getByRole('dialog', { name: downloads.add.title })
    // The address takes the keyboard when the dialog opens.
    expect(getByTestId('direct-add-url').hasAttribute('autofocus')).toBe(true)
  })

  it('opens with `a` while the list is on the page, and not after', async () => {
    const { getByRole, unmount } = mountView()
    pressA()
    await nextTick()
    getByRole('dialog', { name: downloads.add.title })

    unmount()
    expect(openDirectAdd()).toBe(false)
  })

  it('closes once the link is queued', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { id: 'dl-1', state: 'queued' } } as never)
    const { getByTestId, queryByRole, getByRole } = mountView()
    await fireEvent.click(getByTestId('downloads-add'))
    await fireEvent.update(getByTestId('direct-add-url'), 'https://files.example.com/a.zip')
    await fireEvent.submit(getByRole('dialog').querySelector('form')!)

    await waitFor(() => expect(queryByRole('dialog')).toBeNull())
    expect(api.POST).toHaveBeenCalledWith('/api/v1/downloads', { body: expect.objectContaining({ url: 'https://files.example.com/a.zip' }) })
  })

  it('keeps a refusal in the dialog, not in the alert behind it', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: undefined, error: { code: 'x' } } as never)
    const { getByTestId, getByRole } = mountView()
    await fireEvent.click(getByTestId('downloads-add'))
    await fireEvent.update(getByTestId('direct-add-url'), 'ftp://nope')
    await fireEvent.submit(getByRole('dialog').querySelector('form')!)

    await waitFor(() => expect(getByTestId('direct-add-error').textContent).toContain('The address is not supported'))
    getByRole('dialog')
    expect(useTransfersStore().error).toBeNull()
  })
})

describe('the queue notices', () => {
  it('go as a toast that leaves by itself when something went through', async () => {
    const { container } = mountView()
    const store = useTransfersStore()
    store.notice = '2 files reset'
    await nextTick()

    expect(toasts.added).toEqual([expect.objectContaining({ title: '2 files reset', color: 'info' })])
    expect(toasts.added[0]).not.toHaveProperty('duration')
    expect(store.notice).toBeNull()
    expect(container.textContent).not.toContain('2 files reset')
  })

  it('stay as a warning when something was left untouched or refused', async () => {
    mountView()
    const store = useTransfersStore()
    store.warning = '1 package was left untouched – still active: Season 2.'
    await nextTick()

    expect(toasts.added).toEqual([expect.objectContaining({ color: 'warning', duration: 0 })])
    expect(store.warning).toBeNull()
  })

  it('shows the same sentence twice as two toasts', async () => {
    mountView()
    const store = useTransfersStore()
    store.notice = 'Queue paused'
    await nextTick()
    store.notice = 'Queue paused'
    await nextTick()
    expect(toasts.added).toHaveLength(2)
  })
})
