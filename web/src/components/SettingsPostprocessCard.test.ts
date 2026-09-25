/**
 * Both plugin lists on this card come straight from installed plugin manifests — the steps from
 * the post-processing plugins, the upload targets from the storage plugins — and both were read
 * once on mount. Either section hides itself when its list is empty, so a plugin installed
 * elsewhere left the card claiming the feature does not exist, and one removed left a switch
 * that writes a `plugin_steps` entry nothing can run.
 */
import { screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { Settings } from '@/api/types'
import settings from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

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

/** Answers the card's two fetches; both lists are what the tests move around. */
function serve(steps: unknown[], destinations: unknown[]) {
  get.mockImplementation(async (path: string) =>
    path === '/api/v1/postprocess/plugin-steps' ? { data: steps } : { data: destinations }
  )
}

function mount() {
  return mountComponent(SettingsPostprocessCard, {
    messages: { settings },
    props: { modelValue: { ...SETTINGS } },
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
