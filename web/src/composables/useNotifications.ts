import { ref, type Ref } from 'vue'

import { api } from '@/api/client'
import type { NotificationEvent } from '@/api/types'
import { useWebPush } from '@/composables/useWebPush'
import { i18n } from '@/i18n'

import { type HeldNotification, type Notifier, type NotifyOptions, createNotifier } from './desktopNotifier'

export type { NotifyOptions } from './desktopNotifier'

/** Persisted opt-in flag; the browser permission itself lives in the user agent. */
const STORAGE_KEY = 'rd.notifications'
/** The lock whose holder is the one tab that notifies (RD-1260-04). */
const LEADER_LOCK = 'rd.notifications.leader'
/** How long a read of the quiet-hours state is used before it is read again. */
const QUIET_READ_MS = 60_000
/** How many held titles the quiet-hours summary names; the rest it counts. */
const SUMMARY_NAMED = 5

const supported = typeof window !== 'undefined' && 'Notification' in window

function readStored(): boolean {
  try {
    return localStorage.getItem(STORAGE_KEY) === 'true'
  } catch {
    return false
  }
}

function writeStored(value: boolean): void {
  try {
    localStorage.setItem(STORAGE_KEY, String(value))
  } catch {
    // storage unavailable (private mode)
  }
}

function currentPermission(): NotificationPermission {
  return supported ? Notification.permission : 'denied'
}

/** Module-level state so every caller shares one opt-in and one permission value. */
const enabled = ref(supported && readStored())
const permission = ref<NotificationPermission>(currentPermission())

/**
 * Shows one notification. `new Notification` throws where only a service worker may notify —
 * the installed app on Android — and that failure was silent; the worker shows it there.
 */
async function show(title: string, body: string | undefined, tag: string | undefined): Promise<void> {
  const options: NotificationOptions = { ...(body ? { body } : {}), ...(tag ? { tag } : {}) }
  try {
    const notification = new Notification(title, options)
    notification.onclick = () => {
      window.focus()
      notification.close()
    }
    return
  } catch {
    // only a service worker may notify here
  }
  try {
    const registration = await navigator.serviceWorker?.getRegistration()
    await registration?.showNotification(title, { ...options, data: { focus: true } })
  } catch {
    // no worker either: nothing left to show it with
  }
}

let pushRead: Promise<void> | null = null

/** Whether push reaches this browser with `event`: the switch is on here and the event chosen. */
async function pushCarries(event: NotificationEvent): Promise<boolean> {
  const push = useWebPush()
  pushRead ??= push.refresh().catch(() => undefined)
  await pushRead
  return push.state.value === 'on' && (push.events.value.length === 0 || push.events.value.includes(event))
}

let quietRead: { until: number | null, at: number } | null = null

/** When quiet hours let notifications out again, from the power status; `null` while they do. */
async function quietUntil(): Promise<number | null> {
  if (quietRead && Date.now() - quietRead.at < QUIET_READ_MS) return quietRead.until
  let until: number | null = null
  try {
    const response = await api.GET('/api/v1/power/status')
    const value = (response.data as unknown as { notifications_quiet_until?: string | null } | undefined)
      ?.notifications_quiet_until
    const parsed = value ? Date.parse(value) : Number.NaN
    until = Number.isNaN(parsed) ? null : parsed
  } catch {
    // unreadable: notify now rather than hold it without knowing until when
  }
  quietRead = { until, at: Date.now() }
  return until
}

function lead(onLeader: () => void): void {
  const locks = typeof navigator === 'undefined' ? undefined : navigator.locks
  if (!locks?.request) {
    onLeader()
    return
  }
  // Held until the tab closes: the promise never settles.
  locks.request(LEADER_LOCK, () => {
    onLeader()
    return new Promise<void>(() => {})
  }).catch(() => onLeader())
}

function summary(held: readonly HeldNotification[]): HeldNotification {
  const t = i18n.global.t
  const named = held.slice(0, SUMMARY_NAMED).map(entry => entry.title)
  const rest = held.length - named.length
  const more = rest > 0 ? [t('notifications.desktop.quiet_summary_more', { count: rest })] : []
  return {
    title: t('notifications.desktop.quiet_summary_title', { count: held.length }),
    body: [...named, ...more].join('; ')
  }
}

let notifier: Notifier | null = null

function sharedNotifier(): Notifier {
  notifier ??= createNotifier({
    show,
    pushCarries,
    quietUntil,
    lead,
    summary,
    now: () => Date.now(),
    setTimer: (callback, delayMs) => { window.setTimeout(callback, delayMs) }
  })
  return notifier
}

export interface Notifications {
  supported: boolean
  enabled: Ref<boolean>
  permission: Ref<NotificationPermission>
  enable: () => Promise<boolean>
  disable: () => void
  notify: (title: string, body?: string, options?: NotifyOptions) => void
}

/**
 * Desktop notifications for finished transfers and arriving links; opt-in is remembered per
 * browser. One tab of the browser shows them, push carries what it carries (RD-1260-04).
 */
export function useNotifications(): Notifications {
  async function enable(): Promise<boolean> {
    if (!supported) return false
    permission.value = currentPermission()
    if (permission.value === 'default') {
      try {
        permission.value = await Notification.requestPermission()
      } catch {
        permission.value = currentPermission()
      }
    }
    const granted = permission.value === 'granted'
    enabled.value = granted
    writeStored(granted)
    return granted
  }

  function disable(): void {
    enabled.value = false
    writeStored(false)
  }

  function notify(title: string, body?: string, options?: NotifyOptions): void {
    if (!supported || !enabled.value) return
    // Read each time: the permission can be withdrawn in the browser while the tab stays open.
    permission.value = currentPermission()
    if (permission.value !== 'granted') return
    void sharedNotifier().notify(title, body, options)
  }

  // Claims the lead early, so the first notification does not wait for it.
  if (supported && enabled.value) sharedNotifier()

  return { supported, enabled, permission, enable, disable, notify }
}
