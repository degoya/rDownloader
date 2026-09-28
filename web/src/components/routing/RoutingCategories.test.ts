/**
 * The reported case (RD-106-15).
 *
 * "When I pick a category to edit, I do not notice at all that it is active in the form
 * above." The form used to stand above the list, so the heading that changed and the button
 * that turned into "Save" were both off screen by the time a row's pencil was pressed. The
 * form now stands beside the list, the row being edited says so, and the focus moves into
 * the form — the three things this test pins down.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { Category, StorageRoot } from '@/api/types'
import common from '@/locales/en/common.json'
import routing from '@/locales/en/routing.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
const post = vi.fn()
const put = vi.fn()
const patch = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
    PUT: (...args: unknown[]) => put(...args),
    PATCH: (...args: unknown[]) => patch(...args),
    DELETE: vi.fn()
  },
  responseError: vi.fn(() => 'rejected'),
  resultMessage: vi.fn(() => '')
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))

/** The shared event stream, reduced to the one handler this editor registers. */
let pluginEvent: ((event: MessageEvent<string>) => void) | null = null
/**
 * Every channel the screen subscribes to. The name matters as much as the reaction:
 * `Granted::may_observe` hands a subscriber an event only when it holds that event's
 * exact scope, so a screen listening on a channel named for another scope is silently
 * never served.
 */
let subscribedNames: string[] = []
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (handlers: Record<string, (event: MessageEvent<string>) => void>) => {
    subscribedNames = Object.keys(handlers)
    pluginEvent = handlers['postprocess_catalog.changed'] ?? null
    return () => { pluginEvent = null }
  }
}))

const { default: RoutingCategories } = await import('./RoutingCategories.vue')

const ROOT: StorageRoot = {
  id: 'root-1',
  name: 'Downloads',
  path: '/downloads',
  is_default: true,
  minimum_free_bytes: null,
  persistence: 'persistent'
}

function category(id: string, name: string): Category {
  return {
    id,
    name,
    color: '#38BDF8',
    storage_root_id: ROOT.id,
    relative_path: name.toLowerCase(),
    is_default: false,
    postprocess_level: null,
    script: null,
    cleanup_extensions: null,
    recursive_unpack: null,
    sfv_verify: null,
    safe_postproc: null,
    delete_par2: null,
    plugin_steps: null,
    upload_enabled: null,
    upload_remote: null
  } as Category
}

function mount() {
  return mountComponent(RoutingCategories, {
    messages: { routing },
    props: { modelValue: [category('cat-1', 'Films'), category('cat-2', 'Series')], roots: [ROOT], loading: false, loadError: null },
    stubs: { UInputTags: true }
  })
}

/** The row that names a category: the badge and the actions live in the same box. */
function rowOf(name: string): HTMLElement {
  return screen.getByText(name).closest('div.border') as HTMLElement
}

describe('RoutingCategories', () => {
  beforeEach(() => {
    get.mockReset()
    get.mockImplementation(async (path: string) =>
      path === '/api/v1/postprocess/scripts' ? { data: { scripts: [], directory: '/scripts' } } : { data: [] }
    )
  })

  it('marks the row being edited, says so in the form, and moves the focus there', async () => {
    mount()
    expect(screen.getByRole('heading', { level: 3, name: routing.category.form_new })).toBeTruthy()
    expect(screen.queryByText(common.editing)).toBeNull()

    await fireEvent.click(within(rowOf('Series')).getByRole('button', { name: common.actions.edit }))

    expect(screen.getByRole('heading', { level: 3, name: routing.category.form_edit })).toBeTruthy()
    expect(within(rowOf('Series')).getByText(common.editing)).toBeTruthy()
    expect(within(rowOf('Films')).queryByText(common.editing)).toBeNull()
    const name = screen.getByPlaceholderText(routing.category.name_placeholder) as HTMLInputElement
    expect(name.value).toBe('Series')
    await waitFor(() => expect(document.activeElement).toBe(name))

    await fireEvent.click(screen.getByRole('button', { name: common.actions.cancel_edit }))

    expect(screen.getByRole('heading', { level: 3, name: routing.category.form_new })).toBeTruthy()
    expect(screen.queryByText(common.editing)).toBeNull()
  })

  it('titles the list column and counts its rows', () => {
    mount()

    const list = screen.getByRole('heading', { level: 3, name: routing.category.title })
    expect(list).toBeTruthy()
    expect(list.parentElement?.textContent).toContain('2')
  })
})

/**
 * The per-category post-processing override lists the installed step plugins, and the store
 * cached them for the lifetime of the page — its comment said a new step needed a restart
 * anyway, which stopped being true once plugins could be installed while the service ran. So
 * the override kept offering a step that had been removed, and hid one that had just arrived.
 */
describe('RoutingCategories reacting to postprocess_catalog.changed', () => {
  const STEP = { plugin_id: 'rd-plugin-tag', name: 'Tagger', version: '0.3.0' }

  /** Answers the editor's two store reads; `steps` is what the test moves around. */
  function serve(steps: unknown[]) {
    get.mockImplementation(async (path: string) => {
      if (path === '/api/v1/postprocess/scripts') return { data: { scripts: [], directory: '/scripts' } }
      if (path === '/api/v1/postprocess/plugin-steps') return { data: steps }
      return { data: [] }
    })
  }

  function overrideSwitch(): HTMLElement | null {
    return screen.queryByRole('switch', { name: routing.category.plugin_steps_override_label })
  }

  beforeEach(() => {
    get.mockReset()
    pluginEvent = null
    subscribedNames = []
  })

  /**
   * The channel, not just the reaction. `/api/v1/postprocess/plugin-steps` costs `Queue`, and a subscriber is
   * handed an event only when it holds that event's exact scope — scopes widen towards `Read`
   * only, so `plugin.changed` would be a subscription the service can never serve. Naming the
   * set exactly also keeps the screen from listening to all three and hiding the next such
   * mistake.
   */
  it('subscribes at the scope its own data is read at', async () => {
    serve([])

    mount()

    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/postprocess/plugin-steps'))
    expect(subscribedNames).toEqual(['postprocess_catalog.changed'])
  })

  it('offers the step override once a step plugin is installed elsewhere', async () => {
    serve([])

    mount()

    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/postprocess/plugin-steps'))
    expect(overrideSwitch()).toBeNull()

    serve([STEP])
    pluginEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)

    await waitFor(() => expect(overrideSwitch()).toBeTruthy(), { timeout: 2000 })
  })

  it('drops the step override when the last step plugin is removed elsewhere', async () => {
    serve([STEP])

    mount()

    await waitFor(() => expect(overrideSwitch()).toBeTruthy())

    // Saving the category now would write a `plugin_steps` list naming a plugin that is no
    // longer installed, which is the reason the cache has to be discarded rather than kept.
    serve([])
    pluginEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)

    await waitFor(() => expect(overrideSwitch()).toBeNull(), { timeout: 2000 })
  })

  it('coalesces a burst of events into one read', async () => {
    serve([STEP])

    mount()

    await waitFor(() => expect(overrideSwitch()).toBeTruthy())
    get.mockClear()
    const event = { data: JSON.stringify({ payload: {} }) } as MessageEvent<string>
    for (let index = 0; index < 5; index += 1) pluginEvent?.(event)

    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/postprocess/plugin-steps'), { timeout: 2000 })
    expect(get.mock.calls.filter(([path]) => path === '/api/v1/postprocess/plugin-steps').length).toBe(1)
  })
})

const SECOND_ROOT: StorageRoot = { ...ROOT, id: 'root-2', name: 'Archive', path: '/mnt/archive', is_default: false }
const EMPTY_ROOT: StorageRoot = { ...ROOT, id: 'root-3', name: 'Scratch', path: '/scratch', is_default: false }

function on(root: StorageRoot, entry: Category): Category {
  return { ...entry, storage_root_id: root.id }
}

function mountWith(modelValue: Category[], roots: StorageRoot[]) {
  return mountComponent(RoutingCategories, {
    messages: { routing },
    props: { modelValue, roots, loading: false, loadError: null },
    stubs: { UInputTags: true }
  })
}

/** The accordion section of one storage root, found by its header button. */
function sectionOf(rootName: string): HTMLElement {
  return screen.getByRole('button', { name: new RegExp(rootName) }).closest('[data-accordion-item]') as HTMLElement
}

function serveEditor(): void {
  get.mockReset()
  get.mockImplementation(async (path: string) =>
    path === '/api/v1/postprocess/scripts' ? { data: { scripts: [], directory: '/scripts' } } : { data: [] }
  )
}

/**
 * RD-150-13: with categories on several drives, the list is grouped by storage root, so a
 * category is found by where it lands rather than in one long list.
 */
describe('RoutingCategories grouped by storage root', () => {
  beforeEach(() => {
    serveEditor()
    post.mockReset()
  })

  it('opens one section per root that holds a category, headed by name, path and count', () => {
    mountWith([
      on(ROOT, { ...category('cat-1', 'Films'), is_default: true }),
      on(SECOND_ROOT, category('cat-2', 'Series')),
      on(SECOND_ROOT, category('cat-3', 'Docs'))
    ], [ROOT, SECOND_ROOT, EMPTY_ROOT])

    const sections = screen.getByTestId('category-groups').querySelectorAll('[data-accordion-item]')
    // In the order of the roots, and none for the root without categories.
    expect(Array.from(sections).map(section => section.querySelector('button')?.textContent)).toEqual([
      expect.stringContaining('Downloads'),
      expect.stringContaining('Archive')
    ])
    expect(screen.queryByRole('button', { name: /Scratch/ })).toBeNull()
    const archive = sectionOf('Archive').querySelector('button') as HTMLElement
    expect(archive.textContent).toContain('/mnt/archive')
    expect(archive.textContent).toContain('2 categories')
    // The section holding the default category says so in its header.
    expect(sectionOf('Downloads').querySelector('button')?.textContent).toContain(routing.category.default_badge)
    expect(archive.textContent).not.toContain(routing.category.default_badge)
    // Every section starts open.
    expect(within(sectionOf('Archive')).getByText('Series')).toBeTruthy()
    expect(within(sectionOf('Downloads')).getByText('Films')).toBeTruthy()
  })

  it('stays flat while every category lies on one root', () => {
    mountWith([category('cat-1', 'Films'), category('cat-2', 'Series')], [ROOT, SECOND_ROOT])

    expect(screen.queryByTestId('category-groups')).toBeNull()
    expect(screen.getByText('Films')).toBeTruthy()
    expect(screen.getByText('Series')).toBeTruthy()
  })

  it('opens the section of a category created into a closed one', async () => {
    mountWith([on(ROOT, category('cat-1', 'Films')), on(SECOND_ROOT, category('cat-2', 'Series'))], [ROOT, SECOND_ROOT])
    await fireEvent.click(sectionOf('Archive').querySelector('button') as HTMLElement)
    expect(within(sectionOf('Archive')).queryByText('Series')).toBeNull()

    post.mockResolvedValue({ data: on(SECOND_ROOT, category('cat-9', 'Music')) })
    await fireEvent.update(screen.getByPlaceholderText(routing.category.name_placeholder), 'Music')
    await fireEvent.click(screen.getByRole('button', { name: routing.category.create }))

    await waitFor(() => expect(within(sectionOf('Archive')).getByText('Music')).toBeTruthy())
    expect(within(sectionOf('Archive')).getByText('Series')).toBeTruthy()
  })

  it('names each header control by its root and reports whether it is open', async () => {
    mountWith([on(ROOT, category('cat-1', 'Films')), on(SECOND_ROOT, category('cat-2', 'Series'))], [ROOT, SECOND_ROOT])
    const header = sectionOf('Archive').querySelector('button') as HTMLElement

    expect(header.getAttribute('aria-expanded')).toBe('true')
    await fireEvent.click(header)
    expect(header.getAttribute('aria-expanded')).toBe('false')
  })
})

/**
 * RD-150-12: a category is copied with every setting, never with the default mark or the rules
 * that point at the original, and the copy opens in the form to be changed.
 */
describe('RoutingCategories duplicate', () => {
  beforeEach(() => {
    serveEditor()
    post.mockReset()
    put.mockReset()
    patch.mockReset()
  })

  it('creates the copy with the settings under a free name and opens it in the form', async () => {
    const original: Category = {
      ...category('cat-1', 'Films'),
      is_default: true,
      color: '#FF0000',
      postprocess_level: 'unpack',
      script: 'tag.sh',
      cleanup_extensions: ['nfo'],
      upload_enabled: true,
      upload_remote: 'gdrive:films',
      plugin_steps: ['rd-plugin-tag'],
      seeding: { enabled: true, ratio_milli: 1500, time: { minutes: 90 } }
    } as Category
    let created: Record<string, unknown> = {}
    post.mockImplementation(async (_path: string, { body }: { body: Record<string, unknown> }) => {
      created = { ...original, ...body, id: 'cat-copy', plugin_steps: null, seeding: null }
      return { data: created }
    })
    patch.mockImplementation(async (_path: string, { body }: { body: Record<string, unknown> }) => ({
      data: { ...created, plugin_steps: body.plugin_steps }
    }))
    put.mockResolvedValue({ data: {} })
    mountWith([original, category('cat-2', 'Films (copy)')], [ROOT])

    await fireEvent.click(within(rowOf('Films')).getByRole('button', { name: common.actions.duplicate }))

    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    const body = post.mock.calls[0]?.[1]?.body as Record<string, unknown>
    expect(body).toMatchObject({
      name: 'Films (copy 2)',
      is_default: false,
      color: '#FF0000',
      storage_root_id: ROOT.id,
      relative_path: 'films',
      postprocess_level: 'unpack',
      script: 'tag.sh',
      cleanup_extensions: ['nfo'],
      upload_enabled: true,
      upload_remote: 'gdrive:films'
    })
    // What the create route does not take travels on the routes that set it.
    await waitFor(() => expect(put).toHaveBeenCalled())
    expect(patch.mock.calls[0]?.[1]).toMatchObject({ params: { path: { id: 'cat-copy' } }, body: { plugin_steps: ['rd-plugin-tag'] } })
    expect(put.mock.calls[0]?.[0]).toBe('/api/v1/categories/{id}/seeding')
    expect(put.mock.calls[0]?.[1]).toMatchObject({
      params: { path: { id: 'cat-copy' } },
      body: { enabled: true, ratio: 1.5, time_minutes: 90, time_unlimited: null }
    })

    // The copy stands in the form, marked as the row being edited; the original keeps its mark.
    await waitFor(() => expect(screen.getByRole('heading', { level: 3, name: routing.category.form_edit })).toBeTruthy())
    expect(within(rowOf('Films (copy 2)')).getByText(common.editing)).toBeTruthy()
    expect(within(rowOf('Films (copy 2)')).queryByText(routing.category.default_badge)).toBeNull()
    expect(within(rowOf('Films')).getByText(routing.category.default_badge)).toBeTruthy()
    const name = screen.getByPlaceholderText(routing.category.name_placeholder) as HTMLInputElement
    expect(name.value).toBe('Films (copy 2)')
    expect(screen.getByText(routing.category.duplicated)).toBeTruthy()
  })

  it('leaves the list alone when the server refuses the copy', async () => {
    post.mockResolvedValue({ error: { code: 'category.name_taken' } })
    mountWith([category('cat-1', 'Films')], [ROOT])

    await fireEvent.click(within(rowOf('Films')).getByRole('button', { name: common.actions.duplicate }))

    await waitFor(() => expect(screen.getByText('rejected')).toBeTruthy())
    expect(screen.getAllByText(/^Films/)).toHaveLength(1)
    expect(patch).not.toHaveBeenCalled()
    expect(put).not.toHaveBeenCalled()
  })
})
