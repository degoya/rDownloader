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
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: vi.fn(),
    PUT: vi.fn(),
    PATCH: vi.fn(),
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

    await fireEvent.click(screen.getByRole('button', { name: routing.cancel_edit }))

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
