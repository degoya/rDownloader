/**
 * Enqueuing a large selection (1.9.0 check): 1042 selected links froze the tab for 46 s before
 * the first request left, because every package looked each selected link up in the whole list;
 * and the notice then counted the 521 packages it made as "links".
 */
import { render } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, ref } from 'vue'

import { api } from '@/api/client'
import type { CollectorPackage, LinkCandidate } from '@/api/types'
import type { CollectorEntry } from '@/composables/useGrabberSelection'
import linkgrabber from '@/locales/en/linkgrabber.json'
import { useCollectorStore } from '@/stores/collector'
import { createTestI18n } from '@/test/mount'

import { useGrabberEnqueue } from './useGrabberEnqueue'

const toastAdd = vi.fn()
vi.mock('@/api/client', async importOriginal => ({
  ...await importOriginal<typeof import('@/api/client')>(),
  api: { GET: vi.fn(), POST: vi.fn(), PATCH: vi.fn(), DELETE: vi.fn() }
}))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: toastAdd }), useOverlay: () => ({ create: () => ({ open: vi.fn() }) }) }))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))
vi.mock('@/composables/useReplayConsent', () => ({ useReplayConsent: () => vi.fn() }))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
vi.mock('@/stores/transfers', () => ({ useTransfersStore: () => ({ refresh: vi.fn(async () => {}) }) }))

/** `packages` packages of `perPackage` online links each, as the store and the view hold them. */
function seed(packages: number, perPackage: number): CollectorEntry[] {
  const collector = useCollectorStore()
  collector.refresh = vi.fn(async () => {})
  const entries: CollectorEntry[] = []
  const all: LinkCandidate[] = []
  for (let p = 0; p < packages; p++) {
    const pkg = { id: `pkg-${p}`, name: `Package ${p}` } as CollectorPackage
    const links = Array.from({ length: perPackage }, (_, l) => ({ id: `c-${p}-${l}`, package_id: pkg.id, state: 'online', url: `https://files.example.com/${p}/${l}` }) as LinkCandidate)
    all.push(...links)
    entries.push({ kind: 'collector', id: pkg.id, createdAt: '2026-10-02T10:00:00Z', position: p, package: pkg, candidates: links })
  }
  collector.packages = entries.map(entry => entry.package)
  collector.candidates = all
  return entries
}

const posts = () => vi.mocked(api.POST).mock.calls as unknown as [string, { body: { ids: string[], paused?: boolean, new_package_name?: string } }][]

describe('useGrabberEnqueue, a large selection', () => {
  beforeEach(() => {
    toastAdd.mockClear()
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
    vi.mocked(api.POST).mockReset()
    // One download package per collector package sent, as the server answers.
    vi.mocked(api.POST).mockImplementation((async (_path: string, init: { body: { ids: string[] } }) =>
      ({ data: { created: init.body.ids.map(id => ({ id })), failed: 0, first_error: null, free_download_files: 0 } })) as never)
  })

  it('enqueues 1042 selected links paused at once and counts the links, not the packages', async () => {
    const pinia = createPinia()
    setActivePinia(pinia)
    const entries = seed(521, 2)
    const selected = entries.flatMap(entry => entry.candidates.map(candidate => candidate.id))
    expect(selected).toHaveLength(1042)
    const enqueue = harnessWith({ entries, pinia }, selected)

    const started = performance.now()
    await enqueue.enqueueSelected(true)
    const elapsed = performance.now() - started

    expect(elapsed).toBeLessThan(1000)
    const sent = posts().filter(([path]) => path === '/api/v1/collector/packages/enqueue')
    expect(sent.map(([, init]) => init.body.ids.length)).toEqual([500, 21])
    expect(sent.every(([, init]) => init.body.paused === true)).toBe(true)
    expect(toastAdd).toHaveBeenCalledWith(expect.objectContaining({ title: 'Enqueued 1042 links and 0 NZB imports' }))
  })

  it('splits a partly selected package off in the order on screen', async () => {
    const pinia = createPinia()
    setActivePinia(pinia)
    const entries = seed(3, 4)
    const first = entries[0] as CollectorEntry
    // Displayed order differs from the store's: the view sorted the package's links backwards.
    first.candidates = [...first.candidates].reverse()
    const enqueue = harnessWith({ entries, pinia }, ['c-0-3', 'c-0-1', 'c-1-0', 'c-1-1', 'c-1-2', 'c-1-3'])

    await enqueue.enqueueSelected()

    const moves = posts().filter(([path]) => path === '/api/v1/collector/candidates/move')
    expect(moves.map(([, init]) => init.body.ids)).toEqual([['c-0-3', 'c-0-1']])
  })
})

/** Mounts the composable on a pinia that already holds the seeded store. */
function harnessWith(seeded: { entries: CollectorEntry[], pinia: ReturnType<typeof createPinia> }, selected: string[]) {
  let enqueue: ReturnType<typeof useGrabberEnqueue> | undefined
  const view = {
    groups: ref(seeded.entries),
    nzbGroups: ref([]),
    enqueueableNzbIds: ref<string[]>([]),
    selection: { collectorIds: ref(selected), nzbIds: ref<string[]>([]), clear: vi.fn() },
    sort: ref('manual' as const),
    filterActive: ref(false),
    notice: ref<string | null>(null),
    bulkBusy: ref(false)
  }
  render(defineComponent({ setup() { enqueue = useGrabberEnqueue(view); return () => null } }), {
    global: { plugins: [createTestI18n({ linkgrabber }), seeded.pinia] }
  })
  if (!enqueue) throw new Error('not set up')
  return enqueue
}
