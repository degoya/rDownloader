/**
 * Push on this device (RD-1240-13): the installed app subscribes at the browser's push service,
 * hands the subscription to rDownloader, and the service worker shows what arrives even without
 * an open tab.
 *
 * Browsers offer push only in a secure context — `https`, or `localhost` — so on a plain `http`
 * address the switch says why it cannot be turned on instead of failing. Whether push is on is
 * read from both sides: the browser's subscription and the service's list, so a subscription
 * the service dropped (an expired one, or one deleted for a lost device) shows as off.
 */
import { ref } from 'vue'

import type { NotificationEvent } from '@/api/types'
import type { ServerMessage } from '@/i18n/server'
import {
  deleteWebPushSubscription,
  listWebPushSubscriptions,
  saveWebPushSubscription,
  webPushKey
} from '@/api/webPush'

export type WebPushState = 'insecure' | 'unsupported' | 'denied' | 'off' | 'on'

/** How long the page waits for its service worker before it gives up. */
const WORKER_WAIT_MS = 10_000

/** Module-level, so every caller shares one state. */
const state = ref<WebPushState>('off')
const busy = ref(false)
const error = ref<ServerMessage | string | null>(null)
/** The events this browser wants; empty means every event. */
const events = ref<NotificationEvent[]>([])
let subscriptionId: string | null = null

function environment(): WebPushState | null {
  if (typeof window === 'undefined') return 'unsupported'
  if (!window.isSecureContext) return 'insecure'
  if (!('serviceWorker' in navigator) || !('PushManager' in window) || !('Notification' in window)) {
    return 'unsupported'
  }
  if (Notification.permission === 'denied') return 'denied'
  return null
}

/** `URL-safe base64` to bytes, as `applicationServerKey` takes them. */
export function keyBytes(value: string): Uint8Array<ArrayBuffer> {
  const base64 = value.replace(/-/g, '+').replace(/_/g, '/')
  const binary = atob(base64 + '='.repeat((4 - (base64.length % 4)) % 4))
  const bytes = new Uint8Array(binary.length)
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index)
  return bytes
}

function sameKey(current: ArrayBuffer | null | undefined, wanted: Uint8Array): boolean {
  if (!current) return false
  const bytes = new Uint8Array(current)
  return bytes.length === wanted.length && bytes.every((byte, index) => byte === wanted[index])
}

/** What the device list calls this browser: its name and its system, from the user agent. */
export function deviceName(userAgent: string): string {
  const browser = /Edg\//.test(userAgent) ? 'Edge'
    : /Firefox\//.test(userAgent) ? 'Firefox'
      : /(Chrome|CriOS)\//.test(userAgent) ? 'Chrome'
        : /Safari\//.test(userAgent) ? 'Safari'
          : 'Browser'
  const system = /Android/.test(userAgent) ? 'Android'
    : /iPhone|iPad|iPod/.test(userAgent) ? 'iOS'
      : /Windows/.test(userAgent) ? 'Windows'
        : /Mac OS X|Macintosh/.test(userAgent) ? 'macOS'
          : /Linux|X11/.test(userAgent) ? 'Linux'
            : null
  return system ? `${browser} · ${system}` : browser
}

async function registration(): Promise<ServiceWorkerRegistration> {
  const timeout = new Promise<never>((_, reject) => {
    setTimeout(() => reject(new Error('service worker not ready')), WORKER_WAIT_MS)
  })
  return Promise.race([navigator.serviceWorker.ready, timeout])
}

async function currentSubscription(): Promise<PushSubscription | null> {
  return (await registration()).pushManager.getSubscription()
}

async function save(subscription: PushSubscription): Promise<boolean> {
  const json = subscription.toJSON()
  const answer = await saveWebPushSubscription({
    endpoint: subscription.endpoint,
    keys: { p256dh: json.keys?.p256dh ?? '', auth: json.keys?.auth ?? '' },
    device_name: deviceName(navigator.userAgent),
    events: events.value
  })
  if (!answer.ok) {
    error.value = answer.message
    return false
  }
  subscriptionId = answer.data.id
  events.value = [...answer.data.events]
  return true
}

/** Reads whether push is on here: subscribed in this browser and known to the service. */
async function refresh(): Promise<void> {
  const blocked = environment()
  if (blocked) {
    state.value = blocked
    return
  }
  try {
    const subscription = await currentSubscription()
    if (!subscription) {
      state.value = 'off'
      return
    }
    const listed = await listWebPushSubscriptions()
    const mine = listed.ok ? listed.data.find(entry => entry.endpoint === subscription.endpoint) : undefined
    subscriptionId = mine?.id ?? null
    if (mine) events.value = [...mine.events]
    state.value = mine ? 'on' : 'off'
  } catch {
    state.value = 'off'
  }
}

/** Asks for permission, subscribes with the service's key and hands the subscription over. */
async function enable(): Promise<void> {
  const blocked = environment()
  if (blocked) {
    state.value = blocked
    return
  }
  busy.value = true
  error.value = null
  try {
    if ((await Notification.requestPermission()) !== 'granted') {
      state.value = 'denied'
      return
    }
    const key = await webPushKey()
    if (!key.ok) {
      error.value = key.message
      return
    }
    const applicationServerKey = keyBytes(key.data.public_key)
    const pushManager = (await registration()).pushManager
    let subscription = await pushManager.getSubscription()
    // A subscription made for another key (the service's key was replaced) is refused by the
    // push service; this browser subscribes anew.
    if (subscription && !sameKey(subscription.options.applicationServerKey, applicationServerKey)) {
      await subscription.unsubscribe()
      subscription = null
    }
    subscription ??= await pushManager.subscribe({ userVisibleOnly: true, applicationServerKey })
    if (await save(subscription)) state.value = 'on'
  } catch (cause) {
    error.value = cause instanceof Error ? cause.message : String(cause)
  } finally {
    busy.value = false
  }
}

/** Stops push here: the service forgets this browser, and the browser unsubscribes. */
async function disable(): Promise<void> {
  busy.value = true
  error.value = null
  try {
    const subscription = await currentSubscription()
    if (subscriptionId) {
      const answer = await deleteWebPushSubscription(subscriptionId)
      // Already gone at the service is just as off.
      if (!answer.ok && answer.status !== 404) {
        error.value = answer.message
        return
      }
    }
    await subscription?.unsubscribe()
    subscriptionId = null
    state.value = 'off'
  } catch (cause) {
    error.value = cause instanceof Error ? cause.message : String(cause)
  } finally {
    busy.value = false
  }
}

/** Changes which events reach this browser; stored at once while push is on. */
async function setEvents(chosen: NotificationEvent[]): Promise<void> {
  events.value = [...chosen]
  if (state.value !== 'on') return
  busy.value = true
  error.value = null
  try {
    const subscription = await currentSubscription()
    if (subscription) await save(subscription)
  } catch (cause) {
    error.value = cause instanceof Error ? cause.message : String(cause)
  } finally {
    busy.value = false
  }
}

export function useWebPush() {
  return { state, busy, error, events, refresh, enable, disable, setEvents }
}
