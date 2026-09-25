/**
 * The download queue as a virtualized list (RD-106-12).
 *
 * The queue is a tree — packages with files under them — and it used to render every node of
 * it. These cases hold the two halves of that change apart: what the list must keep doing
 * (keyboard reorder past the edge of the window, the notice line, the announced length) and
 * what it must stop doing (putting the thousand rows nobody is looking at in the document).
 */
import { appendFileSync } from 'node:fs'

import { fireEvent, render } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'
import { createI18n } from 'vue-i18n'

import type { Download, DownloadPackage } from '@/api/types'
import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import torrent from '@/locales/en/torrent.json'
import { useTransfersStore } from '@/stores/transfers'

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
// The dialogs run through Nuxt UI's overlay, which only exists inside the app shell.
vi.mock('@nuxt/ui/composables', () => ({
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(false) }) }) }),
  useToast: () => ({ add: vi.fn() })
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
  UDashboardToolbar: { template: '<div><slot name="left" /><slot name="right" /></div>' },
  UDropdownMenu: passthrough,
  UIcon: { template: '<span aria-hidden="true" />' },
  UProgress: { template: '<div role="progressbar" v-bind="$attrs" />' },
  USelect: {
    props: ['modelValue', 'items'],
    emits: ['update:modelValue'],
    template: '<select v-bind="$attrs" :value="modelValue" @change="$emit(\'update:modelValue\', $event.target.value)"><option v-for="item in items" :key="item.value" :value="item.value">{{ item.label }}</option></select>'
  },
  UTooltip: passthrough,
  // Everything on the page that is not the list; none of it is what these cases are about.
  DirectAddForm: true,
  PowerCountdownAlert: true,
  StorageCapacityAlert: true,
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
})
