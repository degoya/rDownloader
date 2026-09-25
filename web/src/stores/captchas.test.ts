import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import { resetEventStream } from '@/composables/useEventStream'

import { useCaptchasStore } from './captchas'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn() },
  responseError: vi.fn(() => 'This captcha is no longer waiting'),
  resultMessage: vi.fn(() => 'Captcha answer submitted')
}))

/** Captures the listener `connectEvents` registers, so a server event can be replayed. */
class EventSourceStub {
  static listeners = new Map<string, (event: MessageEvent<string>) => void>()

  onerror: (() => void) | null = null

  addEventListener(name: string, listener: (event: MessageEvent<string>) => void): void {
    EventSourceStub.listeners.set(name, listener)
  }

  close(): void {}
}

/** One image captcha as the API reports it. */
const CAPTCHA = {
  id: '019d0000-0000-7000-8000-000000000001',
  kind: 'image',
  image: 'data:image/png;base64,Qk0=',
  created_at: '2026-09-02T10:00:00Z',
  expires_at: '2026-09-02T10:03:00Z'
}

/**
 * A server event as it actually arrives: the whole envelope, with the event's own data
 * nested under `payload`. Reading `pending` from the top level silently found nothing, so
 * a captcha appearing while the page was open never showed up.
 */
function envelope(pending: unknown[]): MessageEvent<string> {
  return {
    data: JSON.stringify({
      id: '019d0000-0000-7000-8000-0000000000ff',
      kind: 'captcha_changed',
      occurred_at: '2026-09-02T10:00:00Z',
      payload: { pending }
    })
  } as MessageEvent<string>
}

describe('captchas store: live updates', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    // The event stream is a module-level singleton shared by every store; drop the previous
    // test's subscription so this test's stub actually receives the listener.
    resetEventStream()
    EventSourceStub.listeners.clear()
    vi.stubGlobal('EventSource', EventSourceStub)
    // A malformed event makes the store reload; answer that with the queue it already knows.
    vi.mocked(api.GET).mockResolvedValue({ data: [CAPTCHA] } as never)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('takes the pending list from the event envelope payload', () => {
    const store = useCaptchasStore()
    store.connectEvents()
    const listener = EventSourceStub.listeners.get('captcha.changed')
    expect(listener).toBeDefined()

    listener?.(envelope([CAPTCHA]))

    expect(store.pending).toHaveLength(1)
    expect(store.current?.id).toBe(CAPTCHA.id)

    listener?.(envelope([]))

    expect(store.pending).toHaveLength(0)
    expect(store.current).toBeNull()
  })

  it('keeps the known queue when an event carries no usable list', () => {
    const store = useCaptchasStore()
    store.connectEvents()
    const listener = EventSourceStub.listeners.get('captcha.changed')
    listener?.(envelope([CAPTCHA]))

    // An envelope without the expected list triggers a reload rather than a wrong render;
    // until that answers, the queue on screen stays as it was.
    listener?.({ data: '{"kind":"captcha_changed"}' } as MessageEvent<string>)

    expect(store.pending).toHaveLength(1)
  })
})

describe('captchas store: answering', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('loads the queue the server is holding', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: [CAPTCHA] } as never)
    const store = useCaptchasStore()

    await store.refresh()

    expect(store.current?.id).toBe(CAPTCHA.id)
    expect(store.error).toBeNull()
  })

  it('remembers why the queue could not be loaded', async () => {
    vi.mocked(api.GET).mockResolvedValue({ error: { code: 'captcha.not_waiting' } } as never)
    const store = useCaptchasStore()

    await store.refresh()

    expect(store.error).toBe('This captcha is no longer waiting')
  })

  /** A rejected network call must surface as store state, not an unhandled rejection. */
  it('survives a refresh that never reaches the server', async () => {
    vi.mocked(api.GET).mockRejectedValue(new Error('offline'))
    const store = useCaptchasStore()

    await expect(store.refresh()).resolves.toBeUndefined()

    expect(store.error).toBe('offline')
  })

  it('drops an answered captcha at once instead of waiting for the event', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { code: 'captcha.solved' } } as never)
    const store = useCaptchasStore()
    store.pending = [CAPTCHA] as never

    const outcome = await store.solve(CAPTCHA.id, '42')

    expect(api.POST).toHaveBeenCalledWith('/api/v1/captchas/{id}/solution', {
      params: { path: { id: CAPTCHA.id } },
      body: { token: '42' }
    })
    expect(outcome).toEqual({ ok: true, message: 'Captcha answer submitted' })
    expect(store.pending).toHaveLength(0)
    expect(store.busy).toBe(false)
  })

  it('declines a captcha and reports the outcome', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { code: 'captcha.skipped' } } as never)
    const store = useCaptchasStore()
    store.pending = [CAPTCHA] as never

    const outcome = await store.skip(CAPTCHA.id)

    expect(api.POST).toHaveBeenCalledWith('/api/v1/captchas/{id}/skip', {
      params: { path: { id: CAPTCHA.id } }
    })
    expect(outcome.ok).toBe(true)
    expect(store.pending).toHaveLength(0)
  })

  /**
   * A refused answer — a widget captcha, or one that expired meanwhile — still clears the
   * prompt, because it is not a question the user can usefully be asked again.
   */
  it('reports a refused answer and stops offering that captcha', async () => {
    vi.mocked(api.POST).mockResolvedValue({
      error: { code: 'captcha.widget_needs_solver' }
    } as never)
    const store = useCaptchasStore()
    store.pending = [CAPTCHA] as never

    const outcome = await store.solve(CAPTCHA.id, 'guess')

    expect(outcome.ok).toBe(false)
    expect(store.error).toBe('This captcha is no longer waiting')
    expect(store.pending).toHaveLength(0)
  })
})

describe('captchas store: who can answer a widget', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('keeps the server verdict on the browser extension', async () => {
    vi.mocked(api.GET).mockResolvedValue({
      data: { browser_extension_connected: true, browser_extension_seen_at: '2026-09-16T12:00:00Z' }
    } as never)
    const store = useCaptchasStore()
    expect(store.answerers).toBeNull()

    await store.refreshAnswerers()

    expect(api.GET).toHaveBeenCalledWith('/api/v1/captcha-answerers')
    expect(store.answerers?.browser_extension_connected).toBe(true)
  })

  /** Runs from a timer inside the dialog, so a dead server must not become an unhandled rejection. */
  it('keeps the last verdict when the question cannot be asked', async () => {
    vi.mocked(api.GET).mockRejectedValue(new Error('offline'))
    const store = useCaptchasStore()
    store.answerers = { browser_extension_connected: false }

    await expect(store.refreshAnswerers()).resolves.toBeUndefined()

    expect(store.answerers).toEqual({ browser_extension_connected: false })
  })
})
