/**
 * Downloads and the LinkGrabber build the controls around their lists in one order (RD-1230-02,
 * owner 2026-10-09): the row above the list starts with select all and open/close all, the shared
 * elements follow in the same places, and the selection bar orders the actions both offer the
 * same way. Both views are rendered here and read against `utils/listLayout.ts`, so a view that
 * drifts — or moves the selection count away from its list again — fails.
 */
import { fireEvent, render } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick, type Component } from 'vue'

import type { CollectorPackage, Download, DownloadPackage, LinkCandidate } from '@/api/types'
import downloads from '@/locales/en/downloads.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import torrent from '@/locales/en/torrent.json'
import { useCollectorStore } from '@/stores/collector'
import { useSelectionStore } from '@/stores/selection'
import { useTransfersStore } from '@/stores/transfers'
import { createTestI18n, uiStubs } from '@/test/mount'
import { LIST_BAR_ORDER, SHARED_BULK_ACTIONS } from '@/utils/listLayout'

import DownloadsView from './DownloadsView.vue'
import LinkGrabberView from './LinkGrabberView.vue'

vi.mock('@/api/client', () => ({
  api: {
    GET: vi.fn(async () => ({ data: undefined })),
    POST: vi.fn(async () => ({ data: undefined })),
    PUT: vi.fn(async () => ({ data: undefined })),
    PATCH: vi.fn(async () => ({ data: undefined })),
    DELETE: vi.fn(async () => ({ data: undefined }))
  },
  responseError: vi.fn(),
  errorMessage: vi.fn(),
  resultMessage: vi.fn()
}))
vi.mock('@nuxt/ui/composables', () => ({
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(false) }) }) }),
  useToast: () => ({ add: vi.fn() })
}))
vi.mock('vue-router', async importOriginal => ({
  ...await importOriginal<typeof import('vue-router')>(),
  useRoute: () => ({ path: '/', hash: '', query: {} }),
  useRouter: () => ({ replace: vi.fn(async () => undefined), push: vi.fn() })
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))

const stubs = {
  ...uiStubs,
  // Header and body apart, so a test can tell the page's toolbar from the list.
  UDashboardPanel: { template: '<div><header><slot name="header" /></header><main><slot name="body" /></main></div>' },
  UPopover: { template: '<div><slot /><slot name="content" /></div>' },
  UKbd: { props: ['value'], template: '<kbd>{{ value }}</kbd>' },
  IndexerReviewList: true,
  IndexerSearchDrawer: true,
  SiteRulePickPanel: true,
  NzbHistoryModal: true,
  DirectAddModal: true,
  QueueSummary: true,
  PostprocessQueue: true
}

function seedDownloads(): void {
  const store = useTransfersStore()
  store.packages = [{ id: 'pkg-0', name: 'Package 0', destination: '/d', priority: 'normal', created_at: '2026-09-01T10:00:00Z', kind: 'http' } as unknown as DownloadPackage]
  store.downloads = [0, 1].map(index => ({
    id: `dl-${index}`, package_id: 'pkg-0', file_name: `file-${index}.bin`, url: `https://files.example.com/${index}`,
    state: 'failed', kind: 'http', committed_bytes: '0', total_bytes: '10', created_at: '2026-09-01T10:00:00Z', priority: 'normal', position: index
  }) as unknown as Download)
}

function seedLinkGrabber(): void {
  const store = useCollectorStore()
  store.packages = [{ id: 'cpkg-0', name: 'Collected 0', priority: 'normal', created_at: '2026-09-01T10:00:00Z', position: 0 } as unknown as CollectorPackage]
  store.candidates = [0, 1].map(index => ({
    id: `cand-${index}`, batch_id: 'batch-1', package_id: 'cpkg-0', url: `https://files.example.com/${index}.bin`,
    file_name: `link-${index}.bin`, state: 'online', created_at: '2026-09-01T10:00:00Z', priority: 'normal', position: index
  }) as unknown as LinkCandidate)
}

const VIEWS: [string, Component, () => void][] = [
  ['Downloads', DownloadsView, seedDownloads],
  ['LinkGrabber', LinkGrabberView, seedLinkGrabber]
]

/** Renders a view with its list filled and everything selected through the list's own checkbox. */
async function mountSelected(view: Component, seed: () => void): Promise<HTMLElement> {
  const { container } = render(view, {
    global: { plugins: [createTestI18n({ downloads, linkgrabber, torrent })], stubs: stubs as never }
  })
  seed()
  await nextTick()
  await fireEvent.click(container.querySelector('[data-testid="list-select-all"]') as HTMLElement)
  await nextTick()
  return container as HTMLElement
}

function places(container: Element, attribute: 'listBar' | 'bulkAction'): string[] {
  const selector = attribute === 'listBar' ? '[data-list-bar]' : '[data-bulk-action]'
  // A wrapper that hands its attributes on to the control inside (the checkbox's label) counts once.
  return [...container.querySelectorAll<HTMLElement>(selector)]
    .filter(element => element.parentElement?.closest<HTMLElement>(selector)?.dataset[attribute] !== element.dataset[attribute])
    .map(element => element.dataset[attribute] ?? '')
}

beforeEach(() => {
  setActivePinia(createPinia())
  localStorage.clear()
})

describe.each(VIEWS)('%s around its list', (_name, view, seed) => {
  it('starts the row above the list with select all and open/close all, then the shared elements', async () => {
    const container = await mountSelected(view, seed)
    expect(places(container, 'listBar')).toEqual([...LIST_BAR_ORDER])
  })

  it('keeps select all and its count at the list, not in the page toolbar', async () => {
    const container = await mountSelected(view, seed)
    const checkbox = container.querySelector('[data-testid="list-select-all"]') as HTMLElement
    expect(checkbox.closest('header')).toBeNull()
    // Packages and files (links) apart, by the same rule in both lists (RD-1260-07).
    expect(checkbox.closest('label')?.textContent).toMatch(/2 (files|links) in 1 package selected/)
    // The row, the selection bar and the list follow one another with nothing between them.
    const bar = container.querySelector('[data-testid="queue-list-bar"]') as HTMLElement
    expect(bar.nextElementSibling?.getAttribute('data-testid')).toBe('bulk-action-bar')
    expect(bar.nextElementSibling?.nextElementSibling?.querySelector('[data-testid="queue-column-header"]')).not.toBeNull()
  })

  it('orders the selection bar\'s shared actions the same way, each an icon with a name', async () => {
    const container = await mountSelected(view, seed)
    expect(places(container, 'bulkAction')).toEqual([...SHARED_BULK_ACTIONS])
    const bar = container.querySelector('[data-testid="bulk-action-bar"]') as HTMLElement
    expect(bar.querySelector('[data-testid="bulk-selection"]')?.getAttribute('title')).toMatch(/^2 (files|links) in 1 package selected$/)
    for (const button of bar.querySelectorAll('button')) expect(button.getAttribute('aria-label')).toBeTruthy()
  })
})

/**
 * RD-1260-07 (owner 2026-10-12): the selection names packages and files apart wherever it is
 * counted, and a package counts as soon as one of its rows is ticked, wholly or partly.
 */
describe('the selection count', () => {
  function seedTwoDownloadPackages(): void {
    const store = useTransfersStore()
    store.packages = [0, 1].map(index => ({ id: `pkg-${index}`, name: `Package ${index}`, destination: '/d', priority: 'normal', created_at: '2026-09-01T10:00:00Z', kind: 'http' }) as unknown as DownloadPackage)
    store.downloads = [[0, 0], [0, 1], [1, 0], [1, 1], [1, 2]].map(([pkg, index]) => ({
      id: `dl-${pkg}-${index}`, package_id: `pkg-${pkg}`, file_name: `file-${pkg}-${index}.bin`, url: `https://files.example.com/${pkg}/${index}`,
      state: 'failed', kind: 'http', committed_bytes: '0', total_bytes: '10', created_at: '2026-09-01T10:00:00Z', priority: 'normal', position: index
    }) as unknown as Download)
  }

  function seedTwoGrabberPackages(): void {
    const store = useCollectorStore()
    store.packages = [0, 1].map(index => ({ id: `cpkg-${index}`, name: `Collected ${index}`, priority: 'normal', created_at: '2026-09-01T10:00:00Z', position: index }) as unknown as CollectorPackage)
    store.candidates = [[0, 0], [0, 1], [1, 0], [1, 1], [1, 2]].map(([pkg, index]) => ({
      id: `cand-${pkg}-${index}`, batch_id: 'batch-1', package_id: `cpkg-${pkg}`, url: `https://files.example.com/${pkg}/${index}.bin`,
      file_name: `link-${pkg}-${index}.bin`, state: 'online', created_at: '2026-09-01T10:00:00Z', priority: 'normal', position: index
    }) as unknown as LinkCandidate)
  }

  /** Everything ticked through the list's checkbox, then one row of the second package unticked. */
  async function mountPartly(view: Component, seed: () => void, row: string): Promise<HTMLElement> {
    // The queue's packages start closed; the rows to untick have to be on screen.
    localStorage.setItem('rdownloader-open-packages', JSON.stringify(['pkg-0', 'pkg-1']))
    const container = await mountSelected(view, seed)
    await fireEvent.click(container.querySelector(`[data-row-key="${row}"] input[type="checkbox"]`) as HTMLElement)
    await nextTick()
    return container
  }

  it.each([
    ['Downloads', DownloadsView, seedTwoDownloadPackages, 'file:dl-1-0', '2 packages · 4 files selected'],
    ['LinkGrabber', LinkGrabberView, seedTwoGrabberPackages, 'link:cand-1-0', '2 packages · 4 links selected']
  ] as const)('%s counts a partly ticked package in the list bar, the selection bar and the status bar', async (_name, view, seed, row, expected) => {
    const container = await mountPartly(view, seed, row)
    const checkbox = container.querySelector('[data-testid="list-select-all"]') as HTMLElement
    expect(checkbox.closest('label')?.textContent).toBe(expected)
    expect(container.querySelector('[data-testid="bulk-selection"]')?.getAttribute('title')).toBe(expected)
    expect(useSelectionStore().size?.parts).toMatchObject({ packages: 2, items: 4 })
  })

  it('says in the tooltip what the figures on the red actions count', async () => {
    const container = await mountPartly(DownloadsView, seedTwoDownloadPackages, 'file:dl-1-0')
    const reset = container.querySelector('[data-testid="downloads-bulk-reset"]') as HTMLElement
    expect(reset.textContent?.trim()).toBe('4')
    expect(reset.getAttribute('title')).toBe('Reset 4 files: 4 files of the selection that are not running')
    // Deleting takes whole packages only: the partly ticked one is not among them.
    const remove = container.querySelector('[data-testid="downloads-bulk-delete-packages"]') as HTMLElement
    expect(remove.textContent?.trim()).toBe('1')
    expect(remove.getAttribute('title')).toBe('Delete 1 package: 1 package whose files are all selected')
  })
})
