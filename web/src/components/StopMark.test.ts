/**
 * The queue's stop mark on screen (RD-1210-02): the row menu sets and removes it on a file and on
 * a package, a finished row offers none, the marked row carries a named glyph, and the status bar
 * names the mark and removes it.
 */
import { fireEvent, screen, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: undefined })), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'That download or package is already finished'),
  errorMessage: vi.fn()
}))
const toastAdd = vi.hoisted(() => vi.fn())
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: toastAdd }),
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(null) }) }) })
}))
vi.mock('@/composables/useStagedResolvers', () => ({
  useStagedResolvers: () => ({ stagedFor: () => null, trial: vi.fn() })
}))

import { api } from '@/api/client'
import type { Download, DownloadPackage, QueueStopMark } from '@/api/types'
import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import torrent from '@/locales/en/torrent.json'
import { useQueuePauseStore } from '@/stores/queuePause'
import { axeViolations } from '@/test/axe'
import { mountComponent, passthrough } from '@/test/mount'

import PackageGroup from './PackageGroup.vue'
import TransferCard from './TransferCard.vue'
import TransferRail from './TransferRail.vue'

interface MenuItem { label: string, onSelect?: () => void }

/** The dropdown rendered open, its items as buttons inside a marker. */
const UDropdownMenu = {
  props: ['items'],
  methods: { run(item: MenuItem) { item.onSelect?.() } },
  template: `<div><slot />
    <div data-menu-items>
      <button v-for="item in (items ?? []).flat()" :key="item.label" type="button" @click="run(item)">{{ item.label }}</button>
    </div>
  </div>`
}

const PACKAGE = {
  id: 'package-1', name: 'Some Release', state: 'queued', destination: '/downloads/Some Release',
  category_id: null, priority: 'normal', position: 1, has_password: false, kind: 'http',
  nzb_import_id: null, created_at: '2026-10-08T10:00:00Z', enrichment: []
} as unknown as DownloadPackage

function mark(overrides: Partial<QueueStopMark> = {}): QueueStopMark {
  return { download_id: null, package_id: 'package-1', name: 'Some Release', set_at: '2026-10-08T10:00:00Z', ...overrides }
}

function renderPackage(complete = false) {
  const view = mountComponent(PackageGroup, {
    messages: { downloads, common },
    props: {
      package: PACKAGE, downloads: [], categories: [], selection: 'none', open: false, complete,
      packageRate: 0, packageEta: null, dragging: false, canPause: false, canResume: false, controlBusy: null
    },
    stubs: { UDropdownMenu, UBadge: passthrough }
  })
  return { view, store: useQueuePauseStore() }
}

function renderFile(state: string) {
  const view = mountComponent(TransferCard, {
    messages: { downloads, torrent, common },
    props: {
      download: {
        id: 'd1', package_id: 'package-1', kind: 'http', state, file_name: 'release.rar',
        source: 'https://example.invalid/release.rar', committed_bytes: '0', total_bytes: '100'
      } as unknown as Download
    },
    stubs: { UDropdownMenu, UBadge: passthrough }
  })
  return { view, store: useQueuePauseStore() }
}

/** The row's own menu: the last dropdown, after the package header's priority menu. */
function menu(container: Element): HTMLElement {
  return [...container.querySelectorAll('[data-menu-items]')].at(-1) as HTMLElement
}

beforeEach(() => {
  vi.mocked(api.PUT).mockReset()
  vi.mocked(api.DELETE).mockReset()
  toastAdd.mockReset()
})

describe('the stop mark on a package header', () => {
  it('is offered while the package is unfinished and set for the package', async () => {
    vi.mocked(api.PUT).mockResolvedValue({ data: mark() } as never)
    const { view, store } = renderPackage()

    await fireEvent.click(within(menu(view.container)).getByRole('button', { name: 'Set stop mark here' }))

    expect(api.PUT).toHaveBeenCalledWith('/api/v1/queue/stop-mark', { body: { package_id: 'package-1' } })
    expect(store.stopMark?.package_id).toBe('package-1')
    await nextTick()
    expect(screen.getByLabelText('Stop mark')).toBeTruthy()
  })

  it('is not offered on a finished package', () => {
    const { view } = renderPackage(true)
    expect(within(menu(view.container)).queryByRole('button', { name: 'Set stop mark here' })).toBeNull()
  })

  it('carries a named glyph while marked and is removed from the menu', async () => {
    vi.mocked(api.DELETE).mockResolvedValue({ data: { cleared: true } } as never)
    const { view, store } = renderPackage()
    store.stopMark = mark()
    await nextTick()

    expect(screen.getByLabelText('Stop mark').getAttribute('icon')).toBe('i-lucide-octagon-pause')
    expect(await axeViolations(view.container)).toBe('')
    await fireEvent.click(within(menu(view.container)).getByRole('button', { name: 'Remove stop mark' }))

    expect(api.DELETE).toHaveBeenCalledWith('/api/v1/queue/stop-mark')
    expect(store.stopMark).toBeNull()
  })
})

describe('the stop mark on a file row', () => {
  it('is set for the file and shows its glyph', async () => {
    vi.mocked(api.PUT).mockResolvedValue({ data: mark({ download_id: 'd1', package_id: null, name: 'release.rar' }) } as never)
    const { view } = renderFile('queued')

    await fireEvent.click(within(menu(view.container)).getByRole('button', { name: 'Set stop mark here' }))

    expect(api.PUT).toHaveBeenCalledWith('/api/v1/queue/stop-mark', { body: { download_id: 'd1' } })
    await nextTick()
    expect(screen.getByLabelText('Stop mark')).toBeTruthy()
  })

  it('is not offered on a file that is already done', () => {
    for (const state of ['completed', 'failed', 'cancelled', 'skipped', 'seeding']) {
      const { view } = renderFile(state)
      expect(within(menu(view.container)).queryByRole('button', { name: 'Set stop mark here' }), state).toBeNull()
      view.unmount()
    }
  })

  it('says why a refused mark was not set', async () => {
    vi.mocked(api.PUT).mockResolvedValue({ error: { code: 'queue.stop_mark_target_finished' } } as never)
    const { view, store } = renderFile('paused')

    await fireEvent.click(within(menu(view.container)).getByRole('button', { name: 'Set stop mark here' }))

    await vi.waitFor(() => expect(toastAdd).toHaveBeenCalledWith(expect.objectContaining({
      title: 'The stop mark could not be set',
      description: 'That download or package is already finished',
      color: 'error'
    })))
    expect(store.stopMark).toBeNull()
  })
})

describe('the stop mark in the status bar', () => {
  it('names the mark and removes it', async () => {
    vi.mocked(api.DELETE).mockResolvedValue({ data: { cleared: true } } as never)
    const view = mountComponent(TransferRail, { messages: { downloads }, stubs: { SpeedHistoryChart: true, UBadge: passthrough } })
    expect(screen.queryByTestId('rail-stop-mark')).toBeNull()
    const store = useQueuePauseStore()
    store.stopMark = mark()
    await nextTick()

    const figure = screen.getByTestId('rail-stop-mark')
    expect(figure.textContent).toContain('Some Release')
    expect(within(figure).getByLabelText('Stop mark after “Some Release”: the queue pauses once it is done.')).toBeTruthy()
    expect(await axeViolations(view.container)).toBe('')

    await fireEvent.click(within(figure).getByRole('button', { name: 'Remove stop mark' }))
    expect(api.DELETE).toHaveBeenCalledWith('/api/v1/queue/stop-mark')
    await nextTick()
    expect(screen.queryByTestId('rail-stop-mark')).toBeNull()
  })
})
