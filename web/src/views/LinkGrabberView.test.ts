/**
 * The LinkGrabber as a list (RD-106-12).
 *
 * Same shape as `DownloadsView.test.ts`: the collector is a tree of packages with links under
 * them, and these cases hold what the virtualization must keep (keyboard reorder past the
 * viewport, the notice line, the announced length) apart from what it must drop (every row
 * nobody is looking at).
 */
import { appendFileSync } from 'node:fs'

import { fireEvent, render } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'
import { createI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { CollectorPackage, LinkCandidate } from '@/api/types'
import common from '@/locales/en/common.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import torrent from '@/locales/en/torrent.json'
import { useCollectorStore } from '@/stores/collector'
import { useNzbImportsStore } from '@/stores/nzbImports'

import LinkGrabberView from './LinkGrabberView.vue'

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
vi.mock('@nuxt/ui/composables', () => ({
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(false) }) }) }),
  useToast: () => ({ add: vi.fn() })
}))
vi.mock('vue-router', () => ({
  useRoute: () => ({ query: {} }),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() })
}))
// A candidate row asks which providers have an account, and that lookup subscribes to
// `plugin_catalog.changed`; jsdom has no `EventSource`. What it does with the event is pinned in
// `useAccountProviders.test.ts` — here the view only must not open a stream.
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { common, linkgrabber, torrent } } })

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
  IndexerReviewList: true,
  NzbHistoryModal: true,
  BulkActionBar: { template: '<div><slot /></div>' }
}

/** See `DownloadsView.test.ts`: a passing test's `console.log` never reaches the report. */
function record(line: string): void {
  const target = process.env.RD_MEASURE_LOG
  if (target) appendFileSync(target, `${line}\n`)
  else console.log(line)
}

function makePackage(index: number): CollectorPackage {
  return {
    id: `cpkg-${index}`,
    name: `Collected ${index}`,
    priority: 'normal',
    created_at: `2026-09-01T10:${String(index % 60).padStart(2, '0')}:00Z`,
    position: index
  } as unknown as CollectorPackage
}

function makeCandidate(packageIndex: number, index: number): LinkCandidate {
  return {
    id: `cand-${packageIndex}-${index}`,
    batch_id: 'batch-1',
    package_id: `cpkg-${packageIndex}`,
    url: `https://files.example.com/${packageIndex}/${index}.bin`,
    file_name: `link-${packageIndex}-${index}.bin`,
    state: 'online',
    created_at: '2026-09-01T10:00:00Z',
    priority: 'normal',
    position: index,
    size: '1048576'
  } as unknown as LinkCandidate
}

function seedCollector(packages: number, linksPerPackage: number) {
  const store = useCollectorStore()
  store.packages = Array.from({ length: packages }, (_, index) => makePackage(index))
  store.candidates = Array.from({ length: packages }, (_, packageIndex) =>
    Array.from({ length: linksPerPackage }, (_, index) => makeCandidate(packageIndex, index))).flat()
  return store
}

/** One reviewed NZB import, the shape a hotfolder drop leaves in the LinkGrabber. */
function seedNzbImport(position = 0) {
  const store = useNzbImportsStore()
  store.imports = [{
    id: 'nzb-1',
    name: 'Release.nzb',
    state: 'imported',
    created_at: '2026-09-01T10:00:00Z',
    position,
    file_count: 1,
    segment_count: 1,
    total_bytes: '1048576',
    duplicate: false,
    has_password: false,
    category_id: null,
    priority: 'normal'
  } as unknown as (typeof store.imports)[number]]
  return store
}

function mountView() {
  return render(LinkGrabberView, { global: { plugins: [i18n], stubs } })
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

describe('LinkGrabberView', () => {
  it('shows the packages and their links', async () => {
    seedCollector(2, 3)
    const { container } = mountView()
    await nextTick()
    expect(container.textContent).toContain('Collected 0')
    expect(container.textContent).toContain('link-0-0.bin')
  })

  /** The length a screen reader is told is the list's, not the window's (`docs/accessibility.md`). */
  it('tells a screen reader how long the list is, not how much of it is rendered', async () => {
    seedCollector(50, 10)
    const { container } = mountView()
    await nextTick()

    const list = container.querySelector('[role="list"]')
    expect(list?.getAttribute('aria-label')).toBe('Collected links, 550 rows')
    const rendered = container.querySelectorAll('[role="listitem"]')
    expect(rendered.length).toBeLessThan(550)
    expect(rendered[0]?.getAttribute('aria-setsize')).toBe('550')
  })

  /** A focused handle is pinned, so scrolling the window past it does not drop the keyboard. */
  it('keeps a focused row in the document when the window scrolls past it', async () => {
    seedCollector(1, 400)
    const { container } = mountView()
    await nextTick()

    const handle = handleOf(container, 'link:cand-0-3')
    handle.focus()

    const viewport = container.querySelector<HTMLElement>('[role="list"]')?.parentElement
    if (!viewport) throw new Error('no viewport')
    viewport.scrollTop = 12_000
    await fireEvent.scroll(viewport)
    await settle()

    expect(container.querySelector('[data-row-key="link:cand-0-4"]')).toBeNull()
    expect(container.querySelector('[data-row-key="link:cand-0-3"]')).not.toBeNull()
    expect(document.activeElement).toBe(handle)
  })

  /** The arrow keys keep moving a link once it has left the window it started in. */
  it('reorders with the arrow keys past the edge of the rendered window', async () => {
    const store = seedCollector(1, 400)
    store.reorderCandidates = vi.fn(async (_packageId: string, order: string[]) => {
      const rank = new Map(order.map((id, index) => [id, index]))
      store.candidates = store.candidates.map(candidate =>
        rank.has(candidate.id) ? { ...candidate, position: rank.get(candidate.id) ?? 0 } : candidate)
      return true
    })
    store.refresh = vi.fn(async () => {})

    const { container } = mountView()
    await nextTick()
    handleOf(container, 'link:cand-0-3').focus()

    for (let press = 0; press < 30; press += 1) {
      const current = document.activeElement as HTMLElement | null
      expect(current?.closest('[data-row-key]')?.getAttribute('data-row-key')).toBe('link:cand-0-3')
      await fireEvent.keyDown(current as HTMLElement, { key: 'ArrowDown' })
      await settle()
    }

    expect(container.querySelector('[data-row-key="package:cpkg-0"]')).toBeNull()
    expect(store.reorderCandidates).toHaveBeenCalledTimes(30)
    expect(store.candidates.find(candidate => candidate.id === 'cand-0-3')?.position).toBe(33)
  })

  /** A refused reorder says why in the notice line, exactly as it did before (RD-104-05). */
  it('says why a reorder was refused while a filter is active', async () => {
    const store = seedCollector(1, 4)
    const { container } = mountView()
    await nextTick()

    // The hoster facet is server state since RD-110-19, so the test states the answer the
    // server gave rather than driving a select whose change is a request.
    store.mirrorPreference = { quality: null, language: null, hoster: 'files.example.com', hidden_hosters: [] }
    await nextTick()

    await fireEvent.keyDown(handleOf(container, 'link:cand-0-1'), { key: 'ArrowDown' })
    await settle()

    expect(container.textContent).toContain(linkgrabber.notices.reorder_filter_active)
  })

  /** Shift picks a range in the order the rows are on screen in. */
  it('selects a range of links with shift', async () => {
    seedCollector(1, 8)
    const { container } = mountView()
    await nextTick()

    const boxOf = (id: string) => {
      const box = container.querySelector<HTMLInputElement>(`[data-row-key="link:${id}"] input[type="checkbox"]`)
      if (!box) throw new Error(`no checkbox for ${id}`)
      return box
    }

    await fireEvent.click(boxOf('cand-0-1'))
    await nextTick()
    await fireEvent.click(boxOf('cand-0-5'), { shiftKey: true })
    await nextTick()

    for (const index of [1, 2, 3, 4, 5]) expect(boxOf(`cand-0-${index}`).checked).toBe(true)
    expect(boxOf('cand-0-0').checked).toBe(false)
    expect(boxOf('cand-0-6').checked).toBe(false)
  })

  /**
   * RD-107-09: the paused button checked `groups` — the collector packages — while its
   * neighbour checked `entries`, which is collector packages *and* NZB imports. A LinkGrabber
   * holding nothing but hotfolder NZBs therefore left "add paused" permanently disabled while
   * "add all" worked.
   */
  it('offers "add paused" when the list holds only NZB imports', async () => {
    seedNzbImport()
    const { container } = mountView()
    await nextTick()

    const paused = [...container.querySelectorAll('button')]
      .filter(button => button.textContent?.includes(linkgrabber.actions.enqueue_paused))
    expect(paused.length).toBeGreaterThan(0)
    for (const button of paused) expect(button.disabled).toBe(false)
  })

  /**
   * One manual order across both kinds of entry.
   *
   * The list used to hold two kinds of row that behaved differently for no visible reason: a
   * package had a grip and a stored position, an NZB import had neither and was interleaved by
   * creation time, so it landed wherever its timestamp put it and could not be moved at all.
   */
  describe('the order both kinds share', () => {
    /** Packages at positions 0 and 1, the import after them at 2. */
    function seedMixed() {
      const store = seedCollector(2, 1)
      seedNzbImport(2)
      store.reorderEntries = vi.fn(async () => true)
      return store
    }

    function sectionOf(container: Element, key: string): Element {
      const section = container.querySelector(`[data-row-key="${key}"] section`)
      if (!section) throw new Error(`no row section for ${key}`)
      return section
    }

    it('moves an NZB import with the arrow keys, like the packages around it', async () => {
      const store = seedMixed()
      const { container } = mountView()
      await nextTick()

      await fireEvent.keyDown(handleOf(container, 'nzb:nzb-1'), { key: 'ArrowUp' })
      await settle()

      expect(store.reorderEntries).toHaveBeenCalledWith(
        [{ kind: 'nzb', id: 'nzb-1' }],
        { kind: 'collector', id: 'cpkg-0' }
      )
    })

    /** The drag starts at the handle and the row is the target, exactly as `design.md` has it. */
    it('moves an NZB import by dragging its handle onto a package', async () => {
      const store = seedMixed()
      const { container } = mountView()
      await nextTick()

      await fireEvent.dragStart(handleOf(container, 'nzb:nzb-1'))
      await fireEvent.drop(sectionOf(container, 'package:cpkg-0'))
      await settle()

      // The import moved to the front, so there is nothing to anchor it behind.
      expect(store.reorderEntries).toHaveBeenCalledWith([{ kind: 'nzb', id: 'nzb-1' }], null)
    })

    /**
     * And the other way round — but this one also pins what the request leaves out. The endpoint
     * lifts the listed row out of the order and splices it back behind the anchor, so only the
     * row that moved and the row it now follows need naming: the import at the end is not sent,
     * because it did not move. That is what keeps a request the size of the move rather than the
     * size of the list — a drag three thousand rows down sends exactly this much.
     */
    it('sends only the row that moved and the row it now follows', async () => {
      const store = seedMixed()
      const { container } = mountView()
      await nextTick()

      await fireEvent.dragStart(handleOf(container, 'package:cpkg-0'))
      await fireEvent.drop(sectionOf(container, 'nzb:nzb-1'))
      await settle()

      expect(store.reorderEntries).toHaveBeenCalledWith(
        [{ kind: 'collector', id: 'cpkg-0' }],
        { kind: 'collector', id: 'cpkg-1' }
      )
    })

    /** The refusal that guarded the package order guards the merged one, for both kinds. */
    it.each([
      ['an NZB import', 'nzb:nzb-1'],
      ['a package', 'package:cpkg-0']
    ])('refuses to move %s while a filter hides part of the list, and says why', async (_case, key) => {
      const store = seedMixed()
      const { container } = mountView()
      await nextTick()

      store.mirrorPreference = { quality: null, language: null, hoster: 'files.example.com', hidden_hosters: [] }
      await nextTick()

      await fireEvent.keyDown(handleOf(container, key), { key: 'ArrowDown' })
      await settle()

      expect(store.reorderEntries).not.toHaveBeenCalled()
      expect(container.textContent).toContain(linkgrabber.notices.reorder_filter_active)
    })

    /**
     * A drag under a key sort is applied to the shared manual order and puts the sort back to
     * manual, which is how the package drag has always behaved. What must never happen is the
     * key-sorted sequence being written back as the manual one — that would silently discard
     * the arrangement somebody made by hand.
     */
    it('applies a move to the shared order rather than to what a key sort is showing', async () => {
      const store = seedMixed()
      const { container, getByLabelText } = mountView()
      await nextTick()

      const sortSelect = getByLabelText(linkgrabber.sort.label) as HTMLSelectElement
      await fireEvent.update(sortSelect, 'name')
      await nextTick()

      await fireEvent.keyDown(handleOf(container, 'nzb:nzb-1'), { key: 'ArrowUp' })
      await settle()

      expect(store.reorderEntries).toHaveBeenCalledWith(
        [{ kind: 'nzb', id: 'nzb-1' }],
        { kind: 'collector', id: 'cpkg-0' }
      )
      expect(sortSelect.value).toBe('manual')
    })
  })

  /**
   * RD-110-27: the frame a package carries down to its links actually arrives.
   *
   * `linkFrame()` hands every link row `border-x border-b` as a class, and the row's template
   * had several roots — so Vue could not place the class anywhere and dropped it, silently in
   * production and with a warning nobody read in development.
   */
  it('hands the package frame down to its link rows', async () => {
    seedCollector(1, 2)
    const { container } = mountView()
    await nextTick()

    const row = container.querySelector('[data-row-key="link:cand-0-0"]')?.firstElementChild
    expect(row?.classList.contains('border-x')).toBe(true)
    expect(row?.classList.contains('border-b')).toBe(true)
    expect(row?.querySelector('.queue-row')).not.toBeNull()
  })

  it('measures what a collector of a few thousand links costs', { timeout: 120_000 }, async () => {
    seedCollector(200, 15)
    const started = performance.now()
    const { container } = mountView()
    await nextTick()
    const mounted = performance.now() - started

    const nodes = container.querySelectorAll('*').length
    const rows = container.querySelectorAll('[data-row-key]').length
    record(`[RD-106-12] linkgrabber 200 packages x 15 links: mount ${mounted.toFixed(0)} ms, ${nodes} DOM nodes, ${rows} row wrappers`)

    const store = useCollectorStore()
    const changeStarted = performance.now()
    store.candidates = store.candidates.map((candidate, index) => index === 7 ? { ...candidate, state: 'offline' } : candidate)
    await nextTick()
    record(`[RD-106-12] linkgrabber single row update: ${(performance.now() - changeStarted).toFixed(1)} ms`)

    expect(rows).toBeLessThan(100)
  })
})

describe('LinkGrabberView and mirror groups (RD-110-19)', () => {
  /** A release page's links: one declared group, two qualities, plus one lone link. */
  function seedRelease() {
    const store = useCollectorStore()
    store.packages = [makePackage(0)]
    const member = (index: number, quality: string, selected: boolean) => ({
      ...makeCandidate(0, index),
      file_name: `Show.E01.${quality}.mkv`,
      mirror: { group: 'release', source: 'declared', selected, quality, language: 'German' }
    }) as unknown as LinkCandidate
    store.candidates = [
      member(0, '720p', true),
      member(1, '1080p', false),
      { ...makeCandidate(0, 2), file_name: 'notes.txt' } as unknown as LinkCandidate
    ]
    return store
  }

  // The outcome of the job: the group is one row, not one row per hoster.
  it('draws a mirror group as one row and the rest behind its chevron', async () => {
    seedRelease()
    const { container, getByLabelText } = mountView()
    await nextTick()
    expect(container.textContent).toContain('Show.E01.720p.mkv')
    expect(container.textContent).not.toContain('Show.E01.1080p.mkv')
    expect(container.textContent).toContain('notes.txt')

    await fireEvent.click(getByLabelText(linkgrabber.mirror.expand))
    await settle()
    expect(container.textContent).toContain('Show.E01.1080p.mkv')
  })

  // The preference is server state, so the view asks the server rather than re-sorting itself.
  it('writes a facet to the server and takes the answer back', async () => {
    const store = seedRelease()
    const spy = vi.spyOn(store, 'setMirrorPreference').mockResolvedValue(true)
    const { container } = mountView()
    await nextTick()
    const selects = [...container.querySelectorAll('select')]
    const quality = selects.find(select => select.getAttribute('aria-label') === linkgrabber.filter.quality_label)
    expect(quality).toBeTruthy()
    // The options are the values the list really carries, plus "any".
    expect([...quality!.options].map(option => option.value)).toEqual(['all', '1080p', '720p'])
    await fireEvent.update(quality!, '1080p')
    await settle()
    expect(spy).toHaveBeenCalledWith({ quality: '1080p', language: null, hoster: null, hidden_hosters: [] })
  })

  // "Overridable per package" is the pin: it is a request, and the row offers it.
  it('asks the server to pin the mirror a person picks', async () => {
    const store = seedRelease()
    const spy = vi.spyOn(store, 'chooseMirror').mockResolvedValue(true)
    const { getByLabelText } = mountView()
    await nextTick()
    await fireEvent.click(getByLabelText(linkgrabber.mirror.expand))
    await settle()
    await fireEvent.click(getByLabelText(linkgrabber.mirror.use))
    await settle()
    expect(spy).toHaveBeenCalledWith('cand-0-1', true)
  })

  // A facet hides what cannot satisfy it, and keeps the group that can.
  it('hides a lone link the preference rules out but keeps the group that holds a match', async () => {
    const store = seedRelease()
    store.candidates = [
      ...store.candidates,
      { ...makeCandidate(0, 3), url: 'https://other.example/4.bin', file_name: 'other.bin' } as unknown as LinkCandidate
    ]
    store.mirrorPreference = { quality: null, language: null, hoster: 'files.example.com', hidden_hosters: [] }
    const { container } = mountView()
    await nextTick()
    expect(container.textContent).toContain('Show.E01.720p.mkv')
    expect(container.textContent).not.toContain('other.bin')
  })
})

/**
 * What a filter hides stays in the LinkGrabber (1.2.4).
 *
 * Reported from use: with the hoster facet on one hoster, "add to the queue" sent the links of
 * every other hoster along, because the server was handed whole packages. The view now names
 * the links it shows, and the ones it hid stay behind in their package.
 */
describe('LinkGrabberView adds only what a filter shows', () => {
  /** One package: two links at the filtered hoster, one elsewhere. */
  function seedTwoHosters() {
    const store = useCollectorStore()
    store.packages = [makePackage(0)]
    store.candidates = [
      makeCandidate(0, 0),
      { ...makeCandidate(0, 1), url: 'https://other.example/1.bin', file_name: 'other.bin' } as unknown as LinkCandidate,
      makeCandidate(0, 2)
    ]
    store.refresh = vi.fn(async () => {})
    vi.mocked(api.POST).mockImplementation((async (path: string) => path === '/api/v1/collector/packages/enqueue'
      ? { data: { created: [{ id: 'pkg-1' }], failed: 0, free_download_files: 0 } }
      : { data: undefined }) as unknown as typeof api.POST)
    return store
  }

  function enqueueBody() {
    const calls = vi.mocked(api.POST).mock.calls as unknown as [string, { body?: Record<string, unknown> }?][]
    return calls.find(([path]) => path === '/api/v1/collector/packages/enqueue')?.[1]?.body
  }

  /** The toolbar's actions sit in the navbar's `right` slot, which the shared stub leaves out. */
  function mountWithToolbar() {
    const navbar = { template: '<div><slot name="right" /></div>' }
    return render(LinkGrabberView, { global: { plugins: [i18n], stubs: { ...stubs, UDashboardNavbar: navbar } } })
  }

  function buttonLabelled(container: Element, label: string): HTMLButtonElement {
    const button = [...container.querySelectorAll('button')].find(item => item.textContent?.trim() === label)
    if (!button) throw new Error(`no button "${label}"`)
    return button
  }

  beforeEach(() => {
    vi.mocked(api.POST).mockClear()
    vi.mocked(api.GET).mockImplementation((async () => ({ data: [] })) as unknown as typeof api.GET)
  })

  it('sends only the links the hoster filter shows with "enqueue all", and keeps the rest', async () => {
    const store = seedTwoHosters()
    const { container } = mountWithToolbar()
    await settle()
    // Set once the mount's own preference read has landed, which would otherwise clear it.
    store.mirrorPreference = { quality: null, language: null, hoster: 'files.example.com', hidden_hosters: [] }
    await nextTick()
    expect(container.textContent).not.toContain('other.bin')

    await fireEvent.click(buttonLabelled(container, linkgrabber.actions.enqueue_all))
    await settle()

    expect(enqueueBody()).toEqual({ ids: ['cpkg-0'], paused: false, candidate_ids: ['cand-0-0', 'cand-0-2'] })
    expect(store.candidates.map(candidate => candidate.id)).toEqual(['cand-0-1'])
    expect(store.packages.map(item => item.id)).toEqual(['cpkg-0'])
  })

  it('sends only the visible links of the one package its own button adds', async () => {
    const store = seedTwoHosters()
    const { container } = mountWithToolbar()
    await settle()
    store.mirrorPreference = { quality: null, language: null, hoster: 'files.example.com', hidden_hosters: [] }
    await nextTick()

    await fireEvent.click(buttonLabelled(container, linkgrabber.actions.enqueue))
    await settle()

    expect(enqueueBody()).toEqual({ ids: ['cpkg-0'], paused: false, candidate_ids: ['cand-0-0', 'cand-0-2'] })
    expect(store.candidates.map(candidate => candidate.id)).toEqual(['cand-0-1'])
  })

  it('sends only what the state filter shows', async () => {
    const store = seedTwoHosters()
    store.candidates = store.candidates.map(candidate => candidate.id === 'cand-0-2' ? { ...candidate, state: 'offline' } : candidate)
    const { container, getByLabelText } = mountWithToolbar()
    await nextTick()
    await fireEvent.update(getByLabelText(linkgrabber.filter.state_label) as HTMLSelectElement, 'online')
    await nextTick()

    await fireEvent.click(buttonLabelled(container, linkgrabber.actions.enqueue_all))
    await settle()

    expect(enqueueBody()).toEqual({ ids: ['cpkg-0'], paused: false, candidate_ids: ['cand-0-0', 'cand-0-1'] })
    expect(store.candidates.map(candidate => candidate.id)).toEqual(['cand-0-2'])
  })

  /**
   * A mirror group and a lone link in one package. The group's second mirror and the lone link
   * are in `state` — a mirror still being checked is a fallback all the same (RD-110-20).
   */
  function seedMirrorGroup(state: LinkCandidate['state']) {
    const store = seedTwoHosters()
    const mirror = (selected: boolean) => ({ group: 'release', source: 'declared', selected, quality: '1080p', language: 'German' })
    store.candidates = [
      { ...makeCandidate(0, 0), mirror: mirror(true) } as unknown as LinkCandidate,
      { ...makeCandidate(0, 1), url: 'https://other.example/1.bin', file_name: 'fallback.bin', state, mirror: mirror(false) } as unknown as LinkCandidate,
      { ...makeCandidate(0, 2), file_name: 'lone.bin', state } as unknown as LinkCandidate
    ]
    return store
  }

  // The state filter acts per link, so it hid the mirror in another state, and 1.2.4 left it
  // behind: the queued download then had nothing to fall back to.
  it('sends a shown link\'s mirror along that the state filter hides, but not a hidden lone link', async () => {
    const store = seedMirrorGroup('checking')
    const { container, getByLabelText } = mountWithToolbar()
    await nextTick()
    await fireEvent.update(getByLabelText(linkgrabber.filter.state_label) as HTMLSelectElement, 'online')
    await nextTick()
    expect(container.textContent).not.toContain('lone.bin')

    await fireEvent.click(buttonLabelled(container, linkgrabber.actions.enqueue_all))
    await settle()

    expect(enqueueBody()).toEqual({ ids: ['cpkg-0'], paused: false, candidate_ids: ['cand-0-0', 'cand-0-1'] })
    expect(store.candidates.map(candidate => candidate.id)).toEqual(['cand-0-2'])
  })

  // Selecting a group's row selects its chosen link, and the split moved only that one into the
  // new package: the fallbacks stayed behind in the old one.
  it('moves a selected mirror group whole into the selection\'s package, without the lone link', async () => {
    const store = seedMirrorGroup('online')
    vi.mocked(api.POST).mockImplementation((async (path: string, init?: { body?: { ids?: string[] } }) => {
      if (path === '/api/v1/collector/packages/enqueue') return { data: { created: [{ id: 'pkg-1' }], failed: 0, free_download_files: 0 } }
      if (path === '/api/v1/collector/candidates/move') {
        // The server splits; the store's own refresh after the move then reads the new state.
        const moved = init?.body?.ids ?? []
        const packages = [...store.packages, { ...makePackage(1), id: 'cpkg-sel' }]
        const candidates = store.candidates.map(c => moved.includes(c.id) ? { ...c, package_id: 'cpkg-sel' } : c)
        vi.mocked(api.GET).mockImplementation((async (getPath: string) => ({
          data: getPath === '/api/v1/collector/packages' ? packages : getPath === '/api/v1/collector/candidates' ? candidates : []
        })) as unknown as typeof api.GET)
        return { data: {} }
      }
      return { data: undefined }
    }) as unknown as typeof api.POST)
    const bulkBar = { emits: ['enqueue'], template: '<div><button type="button" @click="$emit(\'enqueue\')">bulk-enqueue</button><slot /></div>' }
    const { container } = render(LinkGrabberView, { global: { plugins: [i18n], stubs: { ...stubs, BulkActionBar: bulkBar } } })
    await settle()

    const box = container.querySelector<HTMLInputElement>(`[data-row-key="link:cand-0-0"] input[aria-label="${linkgrabber.candidate.select}"]`)
    if (!box) throw new Error('no checkbox for the mirror group')
    await fireEvent.click(box)
    await nextTick()
    await fireEvent.click(buttonLabelled(container, 'bulk-enqueue'))
    await settle()

    const calls = vi.mocked(api.POST).mock.calls as unknown as [string, { body?: Record<string, unknown> }?][]
    const move = calls.find(([path]) => path === '/api/v1/collector/candidates/move')?.[1]?.body
    expect(move).toEqual({ ids: ['cand-0-0', 'cand-0-1'], new_package_name: linkgrabber.selection_package.replace('{name}', 'Collected 0') })
    expect(enqueueBody()).toEqual({ ids: ['cpkg-sel'], paused: false })
  })

  /**
   * Hidden hosters (RD-130-21): several at once, stored with the preference, and the same rule
   * as the filters — what the list does not show is neither queued nor checked, except a
   * hidden hoster's mirror of a shown link, which goes along as its fallback.
   */
  describe('with hidden hosters', () => {
    const hide = (store: ReturnType<typeof useCollectorStore>, ...hosters: string[]) => {
      store.mirrorPreference = { quality: null, language: null, hoster: null, hidden_hosters: hosters }
    }

    function chip(container: Element, hoster: string): HTMLButtonElement {
      const row = container.querySelector(`[role="group"][aria-label="${linkgrabber.hosters.label}"]`)
      const button = [...(row?.querySelectorAll('button') ?? [])].find(item => item.textContent?.includes(hoster))
      if (!button) throw new Error(`no chip for ${hoster}`)
      return button
    }

    it('lists every hoster with its links and hides one on a click', async () => {
      const store = seedTwoHosters()
      const spy = vi.spyOn(store, 'setMirrorPreference').mockResolvedValue(true)
      const { container } = mountWithToolbar()
      await settle()
      expect(chip(container, 'files.example.com').textContent).toContain('2')
      expect(chip(container, 'other.example').getAttribute('aria-pressed')).toBe('true')

      await fireEvent.click(chip(container, 'other.example'))
      await settle()
      expect(spy).toHaveBeenCalledWith({ quality: null, language: null, hoster: null, hidden_hosters: ['other.example'] })
    })

    it('says how much it hides and shows everything again in one click', async () => {
      const store = seedTwoHosters()
      const spy = vi.spyOn(store, 'setMirrorPreference').mockResolvedValue(true)
      const { container } = mountWithToolbar()
      await settle()
      hide(store, 'other.example')
      await nextTick()
      expect(container.textContent).not.toContain('other.bin')
      expect(chip(container, 'other.example').getAttribute('aria-pressed')).toBe('false')
      expect(container.textContent).toContain('1 link from 1 hoster hidden')

      await fireEvent.click(buttonLabelled(container, linkgrabber.hosters.show_all))
      await settle()
      expect(spy).toHaveBeenCalledWith({ quality: null, language: null, hoster: null, hidden_hosters: [] })
    })

    /** The mirror group of `seedMirrorGroup`, plus a lone link at the mirror's hoster. */
    function seedHiddenMirror() {
      const store = seedMirrorGroup('online')
      store.candidates = [
        ...store.candidates,
        { ...makeCandidate(0, 3), url: 'https://other.example/3.bin', file_name: 'other-lone.bin' } as unknown as LinkCandidate
      ]
      return store
    }

    it('queues the shown links with the hidden hoster\'s mirror, and leaves its lone link behind', async () => {
      const store = seedHiddenMirror()
      const { container } = mountWithToolbar()
      await settle()
      hide(store, 'other.example')
      await nextTick()
      expect(container.textContent).not.toContain('other-lone.bin')

      await fireEvent.click(buttonLabelled(container, linkgrabber.actions.enqueue_all))
      await settle()

      expect(enqueueBody()).toEqual({ ids: ['cpkg-0'], paused: false, candidate_ids: ['cand-0-0', 'cand-0-1', 'cand-0-2'] })
      expect(store.candidates.map(candidate => candidate.id)).toEqual(['cand-0-3'])
    })

    it('checks only what the list shows, with the mirrors behind it', async () => {
      const store = seedHiddenMirror()
      const { container } = mountWithToolbar()
      await settle()
      hide(store, 'other.example')
      await nextTick()

      await fireEvent.click(buttonLabelled(container, linkgrabber.actions.check_links))
      await settle()

      const calls = vi.mocked(api.POST).mock.calls as unknown as [string, { body?: Record<string, unknown> }?][]
      const check = calls.find(([path]) => path === '/api/v1/collector/candidates/check')?.[1]?.body
      expect(check).toEqual({ ids: ['cand-0-0', 'cand-0-1', 'cand-0-2'] })
    })

    // A hoster hidden once, with no link in the list now, must not make the list read as partial.
    it('sends whole packages while the hidden hosters hide nothing', async () => {
      const store = seedTwoHosters()
      const { container } = mountWithToolbar()
      await settle()
      hide(store, 'gone.example')
      await nextTick()
      expect(container.textContent).not.toContain(linkgrabber.hosters.show_all)

      await fireEvent.click(buttonLabelled(container, linkgrabber.actions.enqueue_all))
      await settle()
      expect(enqueueBody()).toEqual({ ids: ['cpkg-0'], paused: false })
    })
  })

  it('sends whole packages while no filter hides anything', async () => {
    const store = seedTwoHosters()
    const { container } = mountWithToolbar()
    await nextTick()

    await fireEvent.click(buttonLabelled(container, linkgrabber.actions.enqueue_all))
    await settle()

    expect(enqueueBody()).toEqual({ ids: ['cpkg-0'], paused: false })
    expect(store.candidates).toEqual([])
    expect(store.packages).toEqual([])
  })
})
