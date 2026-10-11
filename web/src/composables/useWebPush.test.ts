/**
 * Push on this device (RD-1240-13): the switch subscribes with the service's key and hands the
 * subscription over, reads "on" only when both the browser and the service know it, stops on
 * both sides, and refuses plainly outside a secure context.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const api = vi.hoisted(() => ({
  webPushKey: vi.fn(),
  listWebPushSubscriptions: vi.fn(),
  saveWebPushSubscription: vi.fn(),
  deleteWebPushSubscription: vi.fn()
}))
vi.mock('@/api/webPush', () => api)

const PUBLIC_KEY = 'BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4'
const ENDPOINT = 'https://push.example.org/wpush/abc'

interface Browser {
  subscribe: ReturnType<typeof vi.fn>
  unsubscribe: ReturnType<typeof vi.fn>
  requestPermission: ReturnType<typeof vi.fn>
  current: { value: unknown }
}

function subscription(key: Uint8Array | null, unsubscribe: ReturnType<typeof vi.fn>) {
  return {
    endpoint: ENDPOINT,
    options: { applicationServerKey: key?.buffer ?? null },
    toJSON: () => ({ endpoint: ENDPOINT, keys: { p256dh: 'p256dh-key', auth: 'auth-secret' } }),
    unsubscribe
  }
}

/** A secure browser with push; `permission` is what the prompt answers. */
function browser(options: { secure?: boolean, permission?: NotificationPermission, existing?: Uint8Array | null } = {}): Browser {
  const unsubscribe = vi.fn(() => Promise.resolve(true))
  const current: { value: unknown } = {
    value: options.existing === undefined ? null : subscription(options.existing, unsubscribe)
  }
  const subscribe = vi.fn((init: { applicationServerKey: Uint8Array }) => {
    current.value = subscription(init.applicationServerKey, unsubscribe)
    return Promise.resolve(current.value)
  })
  const requestPermission = vi.fn(() => Promise.resolve(options.permission ?? 'granted'))
  vi.stubGlobal('isSecureContext', options.secure ?? true)
  vi.stubGlobal('PushManager', function PushManager() {})
  vi.stubGlobal('Notification', { permission: 'default', requestPermission })
  Object.defineProperty(navigator, 'serviceWorker', {
    configurable: true,
    value: { ready: Promise.resolve({ pushManager: { getSubscription: () => Promise.resolve(current.value), subscribe } }) }
  })
  return { subscribe, unsubscribe, requestPermission, current }
}

async function load() {
  vi.resetModules()
  return (await import('./useWebPush')).useWebPush()
}

beforeEach(() => {
  for (const mock of Object.values(api)) mock.mockReset()
  api.webPushKey.mockResolvedValue({ ok: true, data: { public_key: PUBLIC_KEY } })
  api.saveWebPushSubscription.mockImplementation((body: { events: string[] }) =>
    Promise.resolve({ ok: true, data: { id: 'sub-1', endpoint: ENDPOINT, device_name: 'x', events: body.events } }))
  api.deleteWebPushSubscription.mockResolvedValue({ ok: true, data: {} })
})

afterEach(() => {
  vi.unstubAllGlobals()
  Reflect.deleteProperty(navigator, 'serviceWorker')
})

describe('push on this device', () => {
  it('subscribes with the service key and hands the subscription over', async () => {
    const page = browser()
    const push = await load()

    await push.enable()

    expect(page.requestPermission).toHaveBeenCalled()
    const init = page.subscribe.mock.calls[0]?.[0] as { userVisibleOnly: boolean, applicationServerKey: Uint8Array }
    expect(init.userVisibleOnly).toBe(true)
    expect(init.applicationServerKey.length).toBe(65)
    expect(init.applicationServerKey[0]).toBe(4)
    expect(api.saveWebPushSubscription).toHaveBeenCalledWith(expect.objectContaining({
      endpoint: ENDPOINT,
      keys: { p256dh: 'p256dh-key', auth: 'auth-secret' },
      events: []
    }))
    expect(push.state.value).toBe('on')
  })

  it('subscribes anew when the browser holds a subscription for another key', async () => {
    const page = browser({ existing: new Uint8Array([4, 1, 2, 3]) })
    const push = await load()

    await push.enable()

    expect(page.unsubscribe).toHaveBeenCalled()
    expect(page.subscribe).toHaveBeenCalled()
    expect(push.state.value).toBe('on')
  })

  it('stays off when the browser refuses notifications', async () => {
    const page = browser({ permission: 'denied' })
    const push = await load()

    await push.enable()

    expect(page.subscribe).not.toHaveBeenCalled()
    expect(api.saveWebPushSubscription).not.toHaveBeenCalled()
    expect(push.state.value).toBe('denied')
  })

  it('says why outside a secure context and asks nothing', async () => {
    const page = browser({ secure: false })
    const push = await load()

    await push.refresh()
    expect(push.state.value).toBe('insecure')
    await push.enable()

    expect(push.state.value).toBe('insecure')
    expect(page.requestPermission).not.toHaveBeenCalled()
    expect(api.webPushKey).not.toHaveBeenCalled()
  })

  it('reads on only when the service knows this browser', async () => {
    browser({ existing: new Uint8Array(65) })
    api.listWebPushSubscriptions.mockResolvedValue({
      ok: true,
      data: [{ id: 'sub-1', endpoint: ENDPOINT, device_name: 'Phone', events: ['package_failed'] }]
    })
    const push = await load()

    await push.refresh()
    expect(push.state.value).toBe('on')
    expect(push.events.value).toEqual(['package_failed'])

    api.listWebPushSubscriptions.mockResolvedValue({ ok: true, data: [] })
    await push.refresh()
    expect(push.state.value).toBe('off')
  })

  it('stops on both sides, also when the service already forgot it', async () => {
    const page = browser()
    const push = await load()
    await push.enable()
    api.deleteWebPushSubscription.mockResolvedValue({ ok: false, status: 404, message: null })

    await push.disable()

    expect(api.deleteWebPushSubscription).toHaveBeenCalledWith('sub-1')
    expect(page.unsubscribe).toHaveBeenCalled()
    expect(push.state.value).toBe('off')
  })

  it('stores a new choice of events while push is on', async () => {
    browser()
    const push = await load()
    await push.enable()

    await push.setEvents(['backup_failed'])

    expect(api.saveWebPushSubscription).toHaveBeenLastCalledWith(expect.objectContaining({ events: ['backup_failed'] }))
    expect(push.events.value).toEqual(['backup_failed'])
  })
})

describe('the device name', () => {
  it('names the browser and the system', async () => {
    const { deviceName } = await import('./useWebPush')
    expect(deviceName('Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0')).toBe('Firefox · Linux')
    expect(deviceName('Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 Chrome/129.0 Mobile Safari/537.36')).toBe('Chrome · Android')
    expect(deviceName('Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/129.0 Safari/537.36 Edg/129.0')).toBe('Edge · Windows')
    expect(deviceName('Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Version/18.0 Mobile/15E148 Safari/604.1')).toBe('Safari · iOS')
    expect(deviceName('curl/8')).toBe('Browser')
  })
})
