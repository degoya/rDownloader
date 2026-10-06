/**
 * The notice for a page left open across an update (RD-1120-16): the single-page app never
 * fetches `index.html` again by itself, so the old interface ran on while the status bar already
 * named the new version. The page compares its build's version with the service's — on start and
 * whenever the event stream opens again — and offers a reload, never reloading by itself.
 * (jsdom's `location.reload` cannot be replaced, so the button's click is not exercised here.)
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { effectScope, nextTick } from 'vue'

const add = vi.fn()
const remove = vi.fn()
const GET = vi.fn()
let streamOpened: (() => void) | null = null

vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add, remove }) }))
vi.mock('vue-i18n', () => ({ useI18n: () => ({ t: (key: string) => key }) }))
vi.mock('@/api/client', () => ({ api: { GET } }))
vi.mock('@/composables/useEventStream', () => ({
  onEventStreamOpened: (listener: () => void) => {
    streamOpened = listener
    return () => { streamOpened = null }
  }
}))

const { BUILD_VERSION, resetServiceVersionForTests, serviceVersion } = await import('./serviceVersion')
const { useVersionNotice } = await import('./useVersionNotice')

/** Lets the health request and the watcher behind it settle. */
async function settle(): Promise<void> {
  await Promise.resolve()
  await nextTick()
}

function serviceAnswers(version: string): void {
  GET.mockResolvedValue({ data: { status: 'ok', version, service: 'rDownloader' } })
}

describe('the notice for an outdated interface', () => {
  let scope = effectScope()

  beforeEach(() => {
    scope = effectScope()
    add.mockReset()
    remove.mockReset()
    GET.mockReset()
    resetServiceVersionForTests()
  })

  afterEach(() => {
    scope.stop()
  })

  it('stays away while the service runs this build', async () => {
    serviceAnswers(BUILD_VERSION)
    scope.run(() => useVersionNotice())
    await settle()

    expect(GET).toHaveBeenCalledWith('/api/v1/health')
    expect(serviceVersion.value).toBe(BUILD_VERSION)
    expect(add).not.toHaveBeenCalled()
  })

  it('offers a reload when the service answers another version on start', async () => {
    serviceAnswers('99.0.0')
    scope.run(() => useVersionNotice())
    await settle()

    expect(add).toHaveBeenCalledTimes(1)
    const toast = add.mock.calls[0]?.[0]
    expect(toast).toMatchObject({ id: 'interface-outdated', title: 'nav.version.outdated', duration: 0 })
    expect(toast.actions).toHaveLength(1)
    // The reload is the user's: a button, never a reload in the middle of an input.
    expect(toast.actions[0]).toMatchObject({ label: 'nav.version.reload', onClick: expect.any(Function) })
  })

  it('checks again when the event stream reopens after a restart', async () => {
    serviceAnswers(BUILD_VERSION)
    scope.run(() => useVersionNotice())
    await settle()
    expect(add).not.toHaveBeenCalled()

    // The service was updated and restarted; the stream comes back.
    serviceAnswers('99.0.0')
    streamOpened?.()
    await settle()

    expect(GET).toHaveBeenCalledTimes(2)
    expect(add).toHaveBeenCalledTimes(1)
  })
})
