/**
 * Both plugin lists on this card come straight from installed plugin manifests — the steps from
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

const { default: SettingsPostprocessCard } = await import('./SettingsPostprocessCard.vue')

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
  return mountComponent(SettingsPostprocessCard, {
    messages: { settings },
    props: { modelValue: model },
    stubs: { UInputTags: true }
  })
}

function fire(): void {
  pluginEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)
}

describe('SettingsPostprocessCard reacting to postprocess_catalog.changed', () => {
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
describe('SettingsPostprocessCard object storage targets', () => {
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
 * RD-150-11: the card's switches used to be hand-built rows — a label paragraph, a description
 * paragraph and a switch named only by its own `aria-label`. They are `UFormField` rows in the
 * horizontal orientation now, so the field's label names the switch and its description stands
 * beside it, the way Nuxt UI wires them.
 */
describe('SettingsPostprocessCard switch rows', () => {
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
    mountComponent(SettingsPostprocessCard, {
      messages: { settings },
      props: { modelValue: { ...SETTINGS } },
      stubs: { UInputTags: true, UFormField }
    })
    const rows = ['recursive_unpack', 'unpack_to_subfolder', 'unwrap_package_folder', 'direct_unpack', 'sfv_verify', 'safe_postproc', 'delete_par2', 'enable_all_par', 'fail_hopeless_jobs', 'enrichment', 'pause', 'ignore_samples', 'upload'] as const
    for (const key of rows) {
      const entry = settings.postprocess[key]
      const toggle = screen.getByRole('switch', { name: entry.label })
      // No private name: the field's label is the only one, so the two cannot drift apart.
      expect(toggle.hasAttribute('aria-label')).toBe(false)
      const field = toggle.closest('[data-orientation]') as HTMLElement
      expect(field.dataset.orientation).toBe('horizontal')
      expect(field.querySelector('[data-description]')?.textContent).toBe(entry.description)
    }
  })

  /** RD-170-16: off by default, and the switch is what writes the setting. */
  it('switches unpacking into a folder per archive on', async () => {
    serve([], [])
    const model = { ...SETTINGS, unpack_to_subfolder: false } as Settings
    mount(model)

    await fireEvent.click(screen.getByRole('switch', { name: settings.postprocess.unpack_to_subfolder.label }))

    expect(model.unpack_to_subfolder).toBe(true)
  })

  /** RD-1140-01: off by default, and the switch is what writes the setting. */
  it('switches dissolving a folder named like the package on', async () => {
    serve([], [])
    const model = { ...SETTINGS, unwrap_package_folder: false } as Settings
    mount(model)

    await fireEvent.click(screen.getByRole('switch', { name: settings.postprocess.unwrap_package_folder.label }))

    expect(model.unwrap_package_folder).toBe(true)
  })

  /** RD-1100-07: opt-in, and the switch is what writes the setting. */
  it('switches unpacking while downloading on', async () => {
    serve([], [])
    const model = { ...SETTINGS, direct_unpack: false } as Settings
    mount(model)

    await fireEvent.click(screen.getByRole('switch', { name: settings.postprocess.direct_unpack.label }))

    expect(model.direct_unpack).toBe(true)
  })
})

/**
 * RD-1140-08: the largest unpacked size was a text field of raw bytes (`107374182400`); it is
 * edited in GiB and still stored in bytes, and the units of the card stand at their fields.
 */
describe('SettingsPostprocessCard sizes and units', () => {
  const GIB = 1024 ** 3

  beforeEach(() => {
    get.mockReset()
    serve([], [])
  })

  it('shows the stored bytes of the archive limit in GiB, the unit at the field', () => {
    mount({ ...SETTINGS, archive_max_uncompressed_bytes: String(100 * GIB) } as Settings)

    const field = screen.getByLabelText(settings.postprocess.max_bytes) as HTMLInputElement
    expect(field.getAttribute('role')).toBe('spinbutton')
    expect(field.value).toBe('100')
    expect(unitOf(field)).toBe('GiB')
  })

  it('stores a typed size in bytes and falls back to the default once emptied', async () => {
    const model = { ...SETTINGS, archive_max_uncompressed_bytes: String(100 * GIB) } as Settings
    mount(model)
    const field = screen.getByTestId('archive-max-size')

    await fireEvent.update(field, '1.5')
    expect(model.archive_max_uncompressed_bytes).toBe(String(1.5 * GIB))

    await fireEvent.update(field, '')
    expect(model.archive_max_uncompressed_bytes).toBe(String(100 * GIB))
  })

  it('puts MiB and seconds at their fields', () => {
    mount({ ...SETTINGS, script_timeout_seconds: 300 } as Settings)

    expect(unitOf(screen.getByLabelText(settings.postprocess.sample_max.label))).toBe('MiB')
    expect(unitOf(screen.getByLabelText(settings.postprocess.script_timeout.label))).toBe('s')
  })
})
