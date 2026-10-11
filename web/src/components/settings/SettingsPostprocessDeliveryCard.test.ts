/**
 * *Scripts & upload* of post-processing (RD-1240-26), moved out of the pipeline card with its
 * tests. Both plugin lists on this card come straight from installed plugin manifests — the steps from
 * the post-processing plugins, the upload targets from the storage plugins — and both were read
 * once on mount. Either section hides itself when its list is empty, so a plugin installed
 * elsewhere left the card claiming the feature does not exist, and one removed left a switch
 * that writes a `plugin_steps` entry nothing can run.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { Settings } from '@/api/types'
import settings from '@/locales/en/settings.json'
import { mountComponent, unitOf } from '@/test/mount'

const get = vi.fn()
vi.mock('@/api/client', () => ({ api: { GET: (...args: unknown[]) => get(...args) } }))

/** The shared event stream, reduced to the one handler this card registers. */
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

const { default: SettingsPostprocessDeliveryCard } = await import('./SettingsPostprocessDeliveryCard.vue')

const STEP = { plugin_id: 'rd-plugin-tag', name: 'Tagger', version: '0.3.0' }
const DESTINATION = { plugin_id: 'rd-plugin-s3', name: 'S3', version: '0.2.1' }

/** Only the fields this card reads; the rest of the document is not its business. */
const SETTINGS = {
  default_level: 'unpack',
  plugin_steps: [],
  cleanup_extensions: [],
  upload_enabled: true,
  upload_remote: '',
  ignore_samples: false,
  sample_max_bytes: 0,
  rar_tool: 'unrar'
} as unknown as Settings

/** Answers the card's fetches; the plugin lists are what most tests move around. */
function serve(steps: unknown[], destinations: unknown[], storageProfiles: unknown[] = []) {
  get.mockImplementation(async (path: string) => {
    if (path === '/api/v1/postprocess/plugin-steps') return { data: steps }
    if (path === '/api/v1/object-storage/profiles') return { data: storageProfiles }
    return { data: destinations }
  })
}

function mount(model: Settings = { ...SETTINGS }) {
  return mountComponent(SettingsPostprocessDeliveryCard, {
    messages: { settings },
    props: { modelValue: model },
  })
}

function fire(): void {
  pluginEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)
}

describe('SettingsPostprocessDeliveryCard reacting to postprocess_catalog.changed', () => {
  beforeEach(() => {
    get.mockReset()
    pluginEvent = null
    subscribedNames = []
  })

  /**
   * The channel, not just the reaction. `/api/v1/postprocess/plugin-steps` costs `Queue`, and a subscriber is
   * handed an event only when it holds that event's exact scope — scopes widen towards `Read`
   * only, so `plugin_catalog.changed` would be a subscription the service can never serve. Naming the
   * set exactly also keeps the screen from listening to all three and hiding the next such
   * mistake.
   */
  it('subscribes at the scope its own data is read at', async () => {
    serve([], [])

    mount()

    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/postprocess/upload-destinations'))
    expect(subscribedNames).toEqual(['postprocess_catalog.changed'])
  })

  it('shows a step plugin installed elsewhere, and hides one removed elsewhere', async () => {
    serve([], [])

    mount()

    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/postprocess/plugin-steps'))
    expect(screen.queryByText('Tagger v0.3.0')).toBeNull()

    serve([STEP], [])
    fire()

    await waitFor(() => expect(screen.getByText('Tagger v0.3.0')).toBeTruthy(), { timeout: 2000 })

    serve([], [])
    fire()

    await waitFor(() => expect(screen.queryByText('Tagger v0.3.0')).toBeNull(), { timeout: 2000 })
  })

  it('shows an upload destination whose storage plugin was installed elsewhere', async () => {
    serve([], [])

    mount()

    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/postprocess/upload-destinations'))
    expect(screen.queryByText('S3 v0.2.1')).toBeNull()

    serve([], [DESTINATION])
    fire()

    await waitFor(() => expect(screen.getByText('S3 v0.2.1')).toBeTruthy(), { timeout: 2000 })
  })

  it('coalesces a burst of events into one pair of reads', async () => {
    serve([STEP], [DESTINATION])

    mount()

    await waitFor(() => expect(screen.getByText('Tagger v0.3.0')).toBeTruthy())
    get.mockClear()
    for (let index = 0; index < 5; index += 1) fire()

    await waitFor(() => expect(get.mock.calls.length).toBe(2), { timeout: 2000 })
    expect(get.mock.calls.length).toBe(2)
  })
})

/**
 * An object storage profile is an upload target the same way a storage plugin is (RD-150-04):
 * the shortcut writes the prefix, and only enabled profiles are offered.
 */
describe('SettingsPostprocessDeliveryCard object storage targets', () => {
  beforeEach(() => get.mockReset())

  it('offers enabled profiles and writes their upload target', async () => {
    serve([], [], [
      { id: 'p1', name: 'Archive bucket', enabled: true },
      { id: 'p2', name: 'Retired bucket', enabled: false },
      { id: 'p3', name: 'Bound bucket', enabled: true, bucket: 'media-bucket' }
    ])
    const model = { ...SETTINGS }

    mount(model)

    const button = await waitFor(() => screen.getByRole('button', { name: 'Archive bucket' }))
    expect(screen.queryByRole('button', { name: 'Retired bucket' })).toBeNull()
    await fireEvent.click(button)
    expect(model.upload_remote).toBe('object-storage:p1/')
    // A profile bound to a bucket names it, so the target is complete as written.
    await fireEvent.click(screen.getByRole('button', { name: 'Bound bucket' }))
    expect(model.upload_remote).toBe('object-storage:p3/media-bucket/')
  })

  it('offers nothing when the profiles cannot be read', async () => {
    get.mockImplementation(async (path: string) =>
      path === '/api/v1/object-storage/profiles' ? { error: { code: 'auth.forbidden' } } : { data: [] }
    )

    mount()

    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/object-storage/profiles'))
    expect(screen.queryByTestId('upload-object-storage')).toBeNull()
  })
})

/**
 * RD-150-11: the switches are `UFormField` rows in the horizontal orientation, so the field's
 * label names the switch and its description stands beside it, the way Nuxt UI wires them.
 */
describe('SettingsPostprocessDeliveryCard switch rows', () => {
  const UFormField = {
    props: ['label', 'description', 'orientation'],
    template:
      '<div :data-orientation="orientation ?? \'vertical\'"><label v-if="label">{{ label }}<slot /></label><slot v-else />'
      + '<p v-if="description" data-description>{{ description }}</p></div>'
  }

  beforeEach(() => {
    get.mockReset()
    serve([], [])
  })

  it('names each switch by its field and keeps the description in the same row', () => {
    mountComponent(SettingsPostprocessDeliveryCard, {
      messages: { settings },
      props: { modelValue: { ...SETTINGS } },
      stubs: { UFormField }
    })
    for (const key of ['enrichment', 'mcp_scripts_allowed', 'upload'] as const) {
      const entry = settings.postprocess[key]
      const toggle = screen.getByRole('switch', { name: entry.label })
      expect(toggle.hasAttribute('aria-label')).toBe(false)
      const field = toggle.closest('[data-orientation]') as HTMLElement
      expect(field.dataset.orientation).toBe('horizontal')
      expect(field.querySelector('[data-description]')?.textContent).toBe(entry.description)
    }
  })

  /** RD-1190-21: off by default, and only this switch -- no MCP tool -- turns it on. */
  it('lets the person allow scripts for MCP tools', async () => {
    const model = { ...SETTINGS, mcp_scripts_allowed: false } as Settings
    mount(model)

    await fireEvent.click(screen.getByRole('switch', { name: settings.postprocess.mcp_scripts_allowed.label }))

    expect(model.mcp_scripts_allowed).toBe(true)
  })

  it('puts seconds at the script timeout', () => {
    mount({ ...SETTINGS, script_timeout_seconds: 300 } as Settings)

    expect(unitOf(screen.getByLabelText(settings.postprocess.script_timeout.label))).toBe('s')
  })
})
