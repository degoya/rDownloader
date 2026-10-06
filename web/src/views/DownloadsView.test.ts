/**
 * The download queue as a virtualized list (RD-106-12).
 *
 * The queue is a tree — packages with files under them — and it used to render every node of
 * it. These cases hold the two halves of that change apart: what the list must keep doing
 * (keyboard reorder past the edge of the window, the notice line, the announced length) and
 * what it must stop doing (putting the thousand rows nobody is looking at in the document).
 */
import { appendFileSync } from 'node:fs'

import { fireEvent, render, waitFor } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'
import { createI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { Download, DownloadPackage, NzbImport } from '@/api/types'
import { SHORTCUT_DEFINITIONS } from '@/composables/shortcutDefinitions'
import { SEARCH_DEBOUNCE_MS } from '@/composables/useQueueFilter'
import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import torrent from '@/locales/en/torrent.json'
import { useNzbImportsStore } from '@/stores/nzbImports'
import { useTransfersStore } from '@/stores/transfers'
import { axeViolations } from '@/test/axe'
import { uiStubs } from '@/test/mount'
import { setShowNzbHandOver } from '@/utils/nzbHandOver'

import DownloadsView from './DownloadsView.vue'

vi.mock('@/api/client', () => ({
  api: {
    GET: vi.fn(async () => ({ data: [] })),
    POST: vi.fn(async () => ({ data: undefined })),
    PUT: vi.fn(async () => ({ data: undefined })),
    PATCH: vi.fn(async () => ({ data: undefined })),
    DELETE: vi.fn(async () => ({ data: undefined }))
  },
  responseError: vi.fn(),
  errorMessage: vi.fn(),
  resultMessage: vi.fn()
}))
// The dialogs run through Nuxt UI's overlay, which only exists inside the app shell. What was
// asked is kept, and the answer is `false` unless a case says otherwise.
const dialogs = vi.hoisted(() => ({ opened: [] as unknown[], answer: false as unknown }))
vi.mock('@nuxt/ui/composables', () => ({
  useOverlay: () => ({
    create: () => ({
      open: (props: unknown) => {
        dialogs.opened.push(props)
        return { result: Promise.resolve(dialogs.answer) }
      }
    })
  }),
  useToast: () => ({ add: vi.fn() })
}))
// The filter and the search live in the address (RD-190-21); `useQueueFilter.test.ts` holds that
// half, here the address only has to exist.
vi.mock('vue-router', async importOriginal => ({
  ...await importOriginal<typeof import('vue-router')>(),
  useRoute: () => ({ path: '/downloads', hash: '', query: {} }),
  useRouter: () => ({ replace: vi.fn(async () => undefined), push: vi.fn() })
}))
/** What "Copy links" put on the clipboard; the copy itself is `useCopyLinks`'s. */
const copied = vi.hoisted(() => ({ links: [] as string[][] }))
vi.mock('@/composables/useCopyLinks', () => ({
  useCopyLinks: () => async (links: string[]) => {
    copied.links.push([...links])
    return true
  }
}))

// jsdom has no `EventSource`; the handlers are kept so the hand-over cases can announce an
// account (RD-191-13).
const stream = vi.hoisted(() => ({ handlers: {} as Record<string, ((event: MessageEvent) => void)[]> }))
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (registered: Record<string, (event: MessageEvent) => void>) => {
    for (const [name, handler] of Object.entries(registered)) (stream.handlers[name] ??= []).push(handler)
    return () => {}
  }
}))

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { common, downloads, torrent } } })

const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }
const stubs = {
  UAlert: { props: ['description'], template: '<div v-bind="$attrs">{{ description }}</div>' },
  UBadge: passthrough,
  UButton: {
    props: ['label', 'ariaLabel', 'disabled'],
    template: '<button type="button" v-bind="$attrs" :disabled="disabled" :aria-label="ariaLabel">{{ label }}<slot /></button>'
  },
  UCheckbox: {
    props: ['modelValue', 'label', 'ariaLabel'],
    emits: ['update:modelValue'],
    template: '<input type="checkbox" v-bind="$attrs" :aria-label="ariaLabel" :checked="modelValue === true" @change="$emit(\'update:modelValue\', $event.target.checked)" />'
  },
  UDashboardNavbar: passthrough,
  UDashboardPanel: { template: '<div><slot name="header" /><slot name="body" /></div>' },
  UDashboardSidebarCollapse: true,
  UEmpty: uiStubs.UEmpty,
  // Without these four every row logged "Failed to resolve component" with the whole row list
  // in its trace: 750 000 of the web stage's 817 000 log lines (RD-1120-08).
  UFormField: uiStubs.UFormField,
  ULink: uiStubs.ULink,
  UModal: uiStubs.UModal,
  USeparator: uiStubs.USeparator,
  UDashboardToolbar: { template: '<div><slot name="left" /><slot name="right" /></div>' },
  UDropdownMenu: passthrough,
  UIcon: { template: '<span aria-hidden="true" />' },
  UInput: {
    inheritAttrs: false,
    props: ['modelValue', 'ariaLabel', 'placeholder'],
    emits: ['update:modelValue'],
    template: '<div><input v-bind="$attrs" :aria-label="ariaLabel" :placeholder="placeholder" :value="modelValue" @input="$emit(\'update:modelValue\', $event.target.value)" /><slot name="trailing" /></div>'
  },
  UKbd: { props: ['value'], template: '<kbd>{{ value }}</kbd>' },
  UProgress: { props: ['modelValue'], template: '<div role="progressbar" :aria-label="`${modelValue ?? 0}%`" v-bind="$attrs" />' },
  USelect: {
    props: ['modelValue', 'items'],
    emits: ['update:modelValue'],
    template: '<select v-bind="$attrs" :value="modelValue" @change="$emit(\'update:modelValue\', $event.target.value)"><option v-for="item in items" :key="item.value" :value="item.value">{{ item.label }}</option></select>'
  },
  /** A real checkbox with the switch's role, so a test flips it and reads it back (RD-150-19). */
  USwitch: {
    inheritAttrs: false,
    props: ['modelValue', 'label'],
    emits: ['update:modelValue'],
    template: '<label>{{ label }}<input type="checkbox" role="switch" v-bind="$attrs" :checked="modelValue" @change="$emit(\'update:modelValue\', $event.target.checked)" /></label>'
  },
  UTooltip: passthrough,
  // Everything on the page that is not the list; none of it is what these cases are about.
  DirectAddForm: true,
  PowerCountdownAlert: true,
  StorageCapacityAlert: true,
  TorrentKillSwitchAlert: true,
  CollisionPromptsAlert: true,
  PostprocessQueue: true,
  QueueSummary: true,
  BulkActionBar: { template: '<div><slot /></div>' }
}

/**
 * Where a measured number goes.
 *
 * Vitest swallows `console.log` from a passing test, and a measurement that only shows up when
 * the run is red is no measurement. `RD_MEASURE_LOG` names a file to append to instead.
 */
function record(line: string): void {
  const target = process.env.RD_MEASURE_LOG
  if (target) appendFileSync(target, `${line}\n`)
  else console.log(line)
}

function makePackage(index: number): DownloadPackage {
  return {
    id: `pkg-${index}`,
    name: `Package ${index}`,
    destination: `/downloads/pkg-${index}`,
    priority: 'normal',
    created_at: '2026-09-01T10:00:00Z',
    kind: 'http'
  } as unknown as DownloadPackage
}

function makeDownload(packageIndex: number, index: number, state = 'queued'): Download {
  return {
    id: `dl-${packageIndex}-${index}`,
    package_id: `pkg-${packageIndex}`,
    file_name: `file-${packageIndex}-${index}.bin`,
    url: `https://files.example.com/${packageIndex}/${index}.bin`,
    state,
    kind: 'http',
    committed_bytes: '0',
    total_bytes: '1048576',
    created_at: '2026-09-01T10:00:00Z',
    priority: 'normal',
    position: index
  } as unknown as Download
}

/** `packages x files` rows, with every package expanded so the whole tree is in the stream. */
function seedQueue(packages: number, filesPerPackage: number, state = 'queued') {
  const store = useTransfersStore()
  store.packages = Array.from({ length: packages }, (_, index) => makePackage(index))
  store.downloads = Array.from({ length: packages }, (_, packageIndex) =>
    Array.from({ length: filesPerPackage }, (_, index) => makeDownload(packageIndex, index, state))).flat()
  localStorage.setItem('rdownloader-open-packages', JSON.stringify(store.packages.map(pkg => pkg.id)))
  return store
}

function mountView() {
  return render(DownloadsView, { global: { plugins: [i18n], stubs } })
}

/** Lets the handler chain — store write, re-render, focus restore — finish. */
async function settle(): Promise<void> {
  await new Promise(resolve => setTimeout(resolve, 0))
  await nextTick()
}

function handleOf(container: Element, key: string): HTMLElement {
  const handle = container.querySelector<HTMLElement>(`[data-row-key="${key}"] [data-row-handle]`)
  if (!handle) throw new Error(`no row handle for ${key}`)
  return handle
}

beforeEach(() => {
  setActivePinia(createPinia())
  localStorage.clear()
  dialogs.opened = []
  dialogs.answer = false
  copied.links = []
})

describe('DownloadsView', () => {
  it('shows the packages and their files', async () => {
    seedQueue(2, 3)
    const { container } = mountView()
    await nextTick()
    expect(container.textContent).toContain('Package 0')
    expect(container.textContent).toContain('file-0-0.bin')
    // Short list: nothing is held back, so nothing has to be scrolled to.
    expect(container.querySelectorAll('[data-row-key]')).toHaveLength(2 + 6)
  })

  /**
   * A screen reader has to be told how long the list is, because it cannot count what is not
   * there (`docs/accessibility.md`). The name carries the total and every row says where it
   * sits in it.
   */
  it('tells a screen reader how long the list is, not how much of it is rendered', async () => {
    seedQueue(50, 10)
    const { container } = mountView()
    await nextTick()

    const list = container.querySelector('[role="list"]')
    expect(list?.getAttribute('aria-label')).toBe('Download queue, 550 rows')
    const rendered = container.querySelectorAll('[role="listitem"]')
    expect(rendered.length).toBeLessThan(550)
    expect(rendered[0]?.getAttribute('aria-setsize')).toBe('550')
    expect(rendered[0]?.getAttribute('aria-posinset')).toBe('1')
  })

  /**
   * The failure mode this whole job risks: virtualization takes rows out of the document, and
   * a row taken out while one of its buttons has focus takes the focus with it. The arrow-key
   * reorder on a drag handle is a 1.0.5 accessibility promise (WCAG 2.1.1 and 2.5.7), so the
   * focused row is pinned and stays.
   */
  it('keeps a focused row in the document when the window scrolls past it', async () => {
    seedQueue(1, 400)
    const { container } = mountView()
    await nextTick()

    const handle = handleOf(container, 'file:dl-0-3')
    handle.focus()
    expect(document.activeElement).toBe(handle)

    const viewport = container.querySelector<HTMLElement>('[role="list"]')?.parentElement
    if (!viewport) throw new Error('no viewport')
    viewport.scrollTop = 12_000
    await fireEvent.scroll(viewport)
    await settle()

    // The window has moved a long way off, and rows around the old position are gone.
    expect(container.querySelector('[data-row-key="file:dl-0-4"]')).toBeNull()
    // The focused one is still there, and still focused.
    expect(container.querySelector('[data-row-key="file:dl-0-3"]')).not.toBeNull()
    expect(document.activeElement).toBe(handle)
  })

  /**
   * Keyboard reorder past the edge of the window.
   *
   * Thirty presses move the file well beyond what is rendered at scroll position zero. After
   * each one the same handle has to be under the keyboard again, or the second press goes
   * nowhere and the feature is reachable by pointer only.
   */
  it('reorders with the arrow keys past the edge of the rendered window', async () => {
    const store = seedQueue(1, 400)
    // The real action would talk to the API; what matters here is that the row really moves.
    store.reorderDownloads = vi.fn(async (_packageId: string, order: string[]) => {
      const byId = new Map(store.downloads.map(download => [download.id, download]))
      store.downloads = order.flatMap(id => byId.get(id) ?? [])
      return true
    })

    const { container } = mountView()
    await nextTick()
    const handle = handleOf(container, 'file:dl-0-3')
    handle.focus()

    for (let press = 0; press < 30; press += 1) {
      const current = document.activeElement as HTMLElement | null
      expect(current?.closest('[data-row-key]')?.getAttribute('data-row-key')).toBe('file:dl-0-3')
      await fireEvent.keyDown(current as HTMLElement, { key: 'ArrowDown' })
      await settle()
    }

    // The window really travelled with it: the rows it started on are out of the document.
    expect(container.querySelector('[data-row-key="package:pkg-0"]')).toBeNull()
    expect(store.reorderDownloads).toHaveBeenCalledTimes(30)
    expect(store.downloads.findIndex(download => download.id === 'dl-0-3')).toBe(33)
    const focusedRow = (document.activeElement as HTMLElement | null)?.closest('[data-row-key]')
    expect(focusedRow?.getAttribute('data-row-key')).toBe('file:dl-0-3')
  })

  /**
   * A refused reorder still says why (RD-104-05): the view's notice line, never a toast and
   * never a silent `return`. Virtualization must not swallow that on the way.
   */
  it('says why a reorder was refused while a filter is active', async () => {
    const store = seedQueue(1, 4, 'downloading')
    const { container, getByLabelText } = mountView()
    await nextTick()

    await fireEvent.update(getByLabelText(downloads.filters.aria), 'active')
    await nextTick()

    await fireEvent.keyDown(handleOf(container, 'file:dl-0-1'), { key: 'ArrowDown' })
    await settle()

    expect(store.notice).toBe(downloads.notices.reorder_filter_active)
    expect(container.textContent).toContain(downloads.notices.reorder_filter_active)
  })

  /** Shift picks a range in the order the rows are on screen in, not in the store's order. */
  it('selects a range of files with shift', async () => {
    seedQueue(1, 8)
    const { container } = mountView()
    await nextTick()

    const boxOf = (id: string) => {
      const box = container.querySelector<HTMLInputElement>(`[data-row-key="file:${id}"] input[type="checkbox"]`)
      if (!box) throw new Error(`no checkbox for ${id}`)
      return box
    }

    await fireEvent.click(boxOf('dl-0-1'))
    await nextTick()
    await fireEvent.click(boxOf('dl-0-5'), { shiftKey: true })
    await nextTick()

    for (const index of [1, 2, 3, 4, 5]) expect(boxOf(`dl-0-${index}`).checked).toBe(true)
    expect(boxOf('dl-0-0').checked).toBe(false)
    expect(boxOf('dl-0-6').checked).toBe(false)
  })

  /** A row that did not change keeps its element; the list is patched, not rebuilt. */
  it('updates one row without rebuilding the list', async () => {
    const store = seedQueue(1, 200)
    const { container } = mountView()
    await nextTick()

    const untouched = container.querySelector('[data-row-key="file:dl-0-5"]')
    store.downloads = store.downloads.map(download =>
      download.id === 'dl-0-2' ? { ...download, state: 'downloading' } : download)
    await nextTick()

    expect(container.querySelector('[data-row-key="file:dl-0-5"]')).toBe(untouched)
  })

  /**
   * The measurement behind the first acceptance criterion; the numbers are recorded in the job
   * file. What is asserted is the shape of the claim: a few thousand rows must not put a few
   * thousand rows in the document.
   */
  it('measures what a queue of a few thousand files costs', { timeout: 120_000 }, async () => {
    seedQueue(200, 15)
    const started = performance.now()
    const { container } = mountView()
    await nextTick()
    const mounted = performance.now() - started

    const nodes = container.querySelectorAll('*').length
    const rows = container.querySelectorAll('[data-row-key]').length
    record(`[RD-106-12] downloads 200 packages x 15 files: mount ${mounted.toFixed(0)} ms, ${nodes} DOM nodes, ${rows} row wrappers`)

    const store = useTransfersStore()
    const changeStarted = performance.now()
    store.downloads = store.downloads.map((download, index) => index === 7 ? { ...download, state: 'downloading' } : download)
    await nextTick()
    record(`[RD-106-12] downloads single row update: ${(performance.now() - changeStarted).toFixed(1)} ms`)

    expect(rows).toBeLessThan(100)
  })

  it('renders without an axe violation', async () => {
    seedQueue(2, 3)
    const { container } = mountView()
    await settle()
    expect(await axeViolations(container)).toBe('')
  })

})

/**
 * The "Show metadata" switch (RD-150-19). The enricher chips under a package name are a view
 * choice of this browser: off hides them, the fields stay on the package, and a reload keeps it.
 */
describe('DownloadsView metadata switch', () => {
  function seedEnriched() {
    const store = useTransfersStore()
    store.packages = [{
      ...makePackage(0),
      enrichment: [{ name: 'metadata.year', value: '2008', plugin_id: 'metadata-enricher', fetched_at: '2026-09-27T10:00:00Z' }]
    } as unknown as DownloadPackage]
    store.downloads = [makeDownload(0, 0)]
    return store
  }

  const chips = (container: Element) => container.querySelectorAll('[data-testid="enrichment-chip"]')
  const toggle = (container: Element) => {
    const input = container.querySelector<HTMLInputElement>('[data-testid="show-metadata"]')
    if (!input) throw new Error('no metadata switch')
    return input
  }

  it('shows the chips by default, hides them when switched off and keeps that across a reload', async () => {
    const store = seedEnriched()
    const first = mountView()
    await nextTick()
    expect(toggle(first.container).checked).toBe(true)
    expect(chips(first.container)).toHaveLength(1)
    expect(first.container.textContent).toContain('Year: 2008')

    await fireEvent.click(toggle(first.container))
    await settle()
    expect(chips(first.container)).toHaveLength(0)
    expect(store.packages[0]?.enrichment).toHaveLength(1)
    expect(localStorage.getItem('rdownloader-show-metadata-downloads')).toBe('false')
    first.unmount()

    const second = mountView()
    await nextTick()
    expect(toggle(second.container).checked).toBe(false)
    expect(chips(second.container)).toHaveLength(0)

    await fireEvent.click(toggle(second.container))
    await settle()
    expect(chips(second.container)).toHaveLength(1)
  })
})

/**
 * `k` removes the finished packages (RD-180-17): the very action of the "Remove completed
 * packages" menu item, with its question, and only while this page is mounted. Which keypress
 * reaches the handler is `useAppShortcuts.test.ts`'s half.
 */
describe('the `k` shortcut', () => {
  const pressK = SHORTCUT_DEFINITIONS.find(definition => definition.keys === 'k')!.handler

  it('asks the menu item\'s question and then clears the completed packages', async () => {
    const store = seedQueue(1, 2, 'completed')
    const clear = vi.spyOn(store, 'clear').mockResolvedValue(undefined)
    dialogs.answer = true
    mountView()
    await nextTick()

    pressK()
    await settle()

    expect(dialogs.opened).toEqual([expect.objectContaining({ description: downloads.confirm.clear_completed, destructive: true })])
    expect(clear.mock.calls).toEqual([['completed']])
  })

  it('clears nothing when the question is declined', async () => {
    const store = seedQueue(1, 2, 'completed')
    const clear = vi.spyOn(store, 'clear').mockResolvedValue(undefined)
    mountView()
    await nextTick()

    pressK()
    await settle()

    expect(dialogs.opened).toHaveLength(1)
    expect(clear).not.toHaveBeenCalled()
  })

  it('does nothing once the page is left', async () => {
    const store = seedQueue(1, 2, 'completed')
    const clear = vi.spyOn(store, 'clear').mockResolvedValue(undefined)
    dialogs.answer = true
    const view = mountView()
    await nextTick()
    view.unmount()

    pressK()
    await settle()

    expect(dialogs.opened).toEqual([])
    expect(clear).not.toHaveBeenCalled()
  })
})

/**
 * The "Clear list" menu (RD-180-21): the entry that removes every stopped package says so, and
 * the one entry that also stops work in progress is the only red one, in a group of its own.
 */
describe('the clear menu', () => {
  interface MenuItem { label: string, color?: string, onSelect: () => void }
  const menus: MenuItem[][][] = []
  const UDropdownMenu = {
    props: ['items'],
    setup(props: Record<string, unknown>) {
      menus.push(props.items as MenuItem[][])
      return {}
    },
    template: '<div><slot /></div>'
  }

  function mountWithMenus() {
    menus.length = 0
    // The menu sits in the navbar's `right` slot, which the shared stub does not render.
    const UDashboardNavbar = { template: '<div><slot /><slot name="right" /></div>' }
    return render(DownloadsView, { global: { plugins: [i18n], stubs: { ...stubs, UDashboardNavbar, UDropdownMenu } } })
  }

  function clearMenu(): MenuItem[][] {
    const menu = menus.find(groups => groups.flat().some(item => item.label === downloads.header.clear_completed))
    if (!menu) throw new Error('no clear menu')
    return menu
  }

  it('names what each entry removes and keeps one red entry apart from the rest', async () => {
    seedQueue(1, 1, 'completed')
    mountWithMenus()
    await nextTick()

    const menu = clearMenu()
    expect(menu.flat().map(item => item.label)).toEqual([
      downloads.header.clear_completed,
      downloads.header.clear_failed,
      'Remove all stopped packages',
      'Clear the entire list'
    ])
    expect(menu.flat().filter(item => item.color === 'error').map(item => item.label)).toEqual(['Clear the entire list'])
    expect(menu.at(-1)?.map(item => item.label)).toEqual(['Clear the entire list'])
  })

  it('asks with the package count and the active ones, then sends the answer along', async () => {
    const store = seedQueue(3, 1, 'completed')
    store.downloads[0]!.state = 'downloading'
    store.downloads[1]!.state = 'seeding'
    const clear = vi.spyOn(store, 'clear').mockResolvedValue(undefined)
    dialogs.answer = { confirmed: true, deletePartial: true }
    mountWithMenus()
    await nextTick()

    clearMenu().flat().find(item => item.label === 'Clear the entire list')!.onSelect()
    await settle()

    expect(dialogs.opened).toEqual([{ packages: 3, active: 2 }])
    expect(clear.mock.calls).toEqual([['everything', true]])
  })

  it('clears nothing when the confirmation is declined', async () => {
    const store = seedQueue(2, 1, 'downloading')
    const clear = vi.spyOn(store, 'clear').mockResolvedValue(undefined)
    dialogs.answer = { confirmed: false, deletePartial: false }
    mountWithMenus()
    await nextTick()

    clearMenu().flat().find(item => item.label === 'Clear the entire list')!.onSelect()
    await settle()

    expect(dialogs.opened).toEqual([{ packages: 2, active: 2 }])
    expect(clear).not.toHaveBeenCalled()
  })

  it('removes the stopped packages through the plain question, without the key', async () => {
    const store = seedQueue(1, 1, 'failed')
    const clear = vi.spyOn(store, 'clear').mockResolvedValue(undefined)
    dialogs.answer = true
    mountWithMenus()
    await nextTick()

    clearMenu().flat().find(item => item.label === 'Remove all stopped packages')!.onSelect()
    await settle()

    expect(dialogs.opened).toEqual([expect.objectContaining({ description: downloads.confirm.clear_all, confirmKey: undefined })])
    expect(clear.mock.calls).toEqual([['all']])
  })
})

/**
 * The name search and the wider state filter (RD-190-21): what the list keeps, what it says when
 * nothing is left, and the `f` key that puts the keyboard in the field.
 */
describe('DownloadsView search and filter', () => {
  function seedMixed() {
    const store = useTransfersStore()
    store.packages = [makePackage(0), { ...makePackage(1), name: 'Holiday Photos' }]
    store.downloads = [
      makeDownload(0, 0, 'failed'),
      makeDownload(0, 1, 'blocked'),
      makeDownload(0, 2, 'seeding'),
      makeDownload(1, 0, 'paused'),
      makeDownload(1, 1, 'completed')
    ]
    localStorage.setItem('rdownloader-open-packages', JSON.stringify(store.packages.map(pkg => pkg.id)))
    return store
  }

  const rowKeys = (container: Element) => [...container.querySelectorAll('[data-row-key]')].map(row => row.getAttribute('data-row-key'))

  async function typeSearch(field: HTMLElement, text: string): Promise<void> {
    await fireEvent.update(field, text)
    await new Promise(resolve => setTimeout(resolve, SEARCH_DEBOUNCE_MS + 20))
    await nextTick()
  }

  it('offers the failed, paused and seeding filters beside the old ones', () => {
    seedMixed()
    const { getByLabelText } = mountView()
    const options = [...(getByLabelText(downloads.filters.aria) as HTMLSelectElement).options].map(option => option.textContent)
    expect(options).toEqual(['All', 'Active', 'Waiting', 'Paused', 'Failed', 'Seeding', 'Completed'])
  })

  it('keeps the failed and the blocked files under Failed', async () => {
    seedMixed()
    const { container, getByLabelText } = mountView()
    await fireEvent.update(getByLabelText(downloads.filters.aria), 'failed')
    await nextTick()
    expect(rowKeys(container)).toEqual(['package:pkg-0', 'file:dl-0-0', 'file:dl-0-1'])
  })

  it('finds a package by its name and a file by its own', async () => {
    seedMixed()
    const { container, getByLabelText } = mountView()
    const field = getByLabelText(downloads.filters.search_label)

    await typeSearch(field, 'holiday')
    expect(rowKeys(container)).toEqual(['package:pkg-1', 'file:dl-1-0', 'file:dl-1-1'])

    await typeSearch(field, 'FILE-0-2')
    expect(rowKeys(container)).toEqual(['package:pkg-0', 'file:dl-0-2'])
  })

  it('says when the filter hides everything, and resets from there', async () => {
    seedMixed()
    const { container, getByLabelText, getByTestId, getByText } = mountView()
    await typeSearch(getByLabelText(downloads.filters.search_label), 'nothing like this')

    expect(rowKeys(container)).toEqual([])
    expect(getByTestId('downloads-no-match').textContent).toContain(downloads.filters.no_match_title)
    expect(container.textContent).not.toContain(downloads.empty.title)

    await fireEvent.click(getByText(downloads.filters.reset))
    await nextTick()
    expect(rowKeys(container)).toHaveLength(2 + 5)
  })

  it('refuses a reorder while only the search narrows the list', async () => {
    const store = seedMixed()
    const { container, getByLabelText } = mountView()
    await typeSearch(getByLabelText(downloads.filters.search_label), 'file-0')

    await fireEvent.keyDown(handleOf(container, 'file:dl-0-1'), { key: 'ArrowDown' })
    await settle()
    expect(store.notice).toBe(downloads.notices.reorder_filter_active)
  })

  describe('the `f` key', () => {
    const pressF = SHORTCUT_DEFINITIONS.find(definition => definition.keys === 'f')!.handler

    it('puts the keyboard in the search and shows itself on the field', async () => {
      seedMixed()
      const { getByLabelText, getByTestId } = mountView()
      await nextTick()
      pressF()
      expect(document.activeElement).toBe(getByLabelText(downloads.filters.search_label))
      expect(getByTestId('downloads-search').parentElement?.querySelector('kbd')?.textContent).toBe('f')
    })

    it('does nothing once the page is left', async () => {
      seedMixed()
      const view = mountView()
      await nextTick()
      view.unmount()
      pressF()
      expect(document.activeElement).toBe(document.body)
    })
  })
})

/** "Copy links" of a package takes every file of it, whatever the filter hides (RD-190-21). */
describe('DownloadsView copy links', () => {
  interface MenuItem { label: string, onSelect?: () => void }
  const menus: MenuItem[][][] = []
  const UDropdownMenu = {
    props: ['items'],
    setup(props: Record<string, unknown>) {
      menus.push(props.items as MenuItem[][])
      return {}
    },
    template: '<div><slot /></div>'
  }

  it('copies the sources of all the package\'s files', async () => {
    menus.length = 0
    const store = seedQueue(1, 3)
    store.downloads = store.downloads.map(download => ({ ...download, source: `https://files.example.com/${download.id}` }))
    store.downloads[2]!.state = 'completed'
    const { getByLabelText } = render(DownloadsView, { global: { plugins: [i18n], stubs: { ...stubs, UDropdownMenu } } })
    await fireEvent.update(getByLabelText(downloads.filters.aria), 'queued')
    await nextTick()

    const item = menus.flat(2).find(entry => entry.label === common.actions.copy_links)
    item?.onSelect?.()
    await settle()
    expect(copied.links).toEqual([[
      'https://files.example.com/dl-0-0',
      'https://files.example.com/dl-0-1',
      'https://files.example.com/dl-0-2'
    ]])
  })
})

/**
 * RD-191-13, the owner's extension: the NZB behind a package goes to a provider from the
 * package's menu, in any state of the package. The entry shows only while the Downloads switch
 * is on and an account takes NZB files; the badge of a package already handed over stays.
 */
describe('DownloadsView NZB hand-over', () => {
  /** The menu rendered open, its items as buttons, so the entries can be found and picked. */
  const menuStubs = {
    ...stubs,
    UDropdownMenu: {
      props: ['items'],
      template: '<div><slot /><div data-menu-items><button v-for="item in (items ?? []).flat()" :key="item.label" type="button" @click="item.onSelect?.()">{{ item.label }}</button></div></div>'
    }
  }

  async function announceAccounts(accounts: unknown[]): Promise<void> {
    vi.mocked(api.GET).mockImplementation((async (path: string) => {
      if (path === '/api/v1/accounts') return { data: accounts }
      if (path === '/api/v1/remote-jobs/providers') return { data: ['torbox'] }
      if (path === '/api/v1/providers') return { data: [{ slug: 'torbox', display_name: 'TorBox', credentials: 'api_key', kind: 'remote' }] }
      return { data: [] }
    }) as unknown as typeof api.GET)
    for (const handler of stream.handlers['account.changed'] ?? []) handler(new MessageEvent('account.changed', { data: '{}' }))
    await new Promise(resolve => setTimeout(resolve, 400))
  }

  /** One failed NZB package, its import handed to TorBox, beside a package of plain links. */
  function seedNzbPackage(): void {
    const store = seedQueue(2, 1, 'failed')
    store.packages = store.packages.map((pkg, index) => index === 0
      ? { ...pkg, kind: 'usenet', state: 'failed', nzb_import_id: 'nzb-1' } as DownloadPackage
      : pkg)
    useNzbImportsStore().imports = [{
      id: 'nzb-1',
      name: 'Package 0',
      state: 'enqueued',
      handed_over: { remote_job_id: 'job-1', account_id: 'acc-torbox' }
    } as unknown as NzbImport]
  }

  function mountWithMenus() {
    return render(DownloadsView, { global: { plugins: [i18n], stubs: menuStubs } })
  }

  const torbox = { id: 'acc-torbox', label: 'Main', provider: 'torbox', enabled: true }

  afterEach(() => {
    setShowNzbHandOver({})
    vi.mocked(api.GET).mockImplementation((async () => ({ data: [] })) as unknown as typeof api.GET)
    vi.mocked(api.POST).mockClear()
  })

  it('offers the account in the menu of the failed NZB package and hands that package over', async () => {
    seedNzbPackage()
    const { getAllByRole, getByTestId } = mountWithMenus()
    await announceAccounts([torbox])

    await waitFor(() => expect(getAllByRole('button', { name: 'Main · TorBox' })).toHaveLength(1))
    expect(getByTestId('package-handed-over').textContent).toBe('Handed to TorBox')

    vi.mocked(api.POST).mockResolvedValueOnce({ data: undefined, error: { code: 'remote_job.not_claimed' } } as never)
    await fireEvent.click(getAllByRole('button', { name: 'Main · TorBox' })[0]!)
    expect(api.POST).toHaveBeenCalledWith('/api/v1/packages/{id}/remote-job', {
      params: { path: { id: 'pkg-0' } },
      body: { account_id: 'acc-torbox' }
    })
  })

  it('drops the entry when the Downloads switch is off and keeps the badge', async () => {
    seedNzbPackage()
    const { getAllByRole, getByTestId, queryByRole } = mountWithMenus()
    await announceAccounts([torbox])
    await waitFor(() => expect(getAllByRole('button', { name: 'Main · TorBox' })).toHaveLength(1))

    setShowNzbHandOver({ nzb_hand_over_downloads_enabled: false })
    await nextTick()

    expect(queryByRole('button', { name: 'Main · TorBox' })).toBeNull()
    expect(getByTestId('package-handed-over').textContent).toBe('Handed to TorBox')
  })

  it('offers nothing while no account takes NZBs', async () => {
    seedNzbPackage()
    const { getByTestId, queryByRole } = mountWithMenus()
    await announceAccounts([{ id: 'acc-other', label: 'Other', provider: 'realdebrid', enabled: true }])

    expect(queryByRole('button', { name: downloads.package.hand_over })).toBeNull()
    expect(getByTestId('package-handed-over')).toBeTruthy()
  })
})
