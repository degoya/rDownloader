import { ref, type Ref } from 'vue'

/** Persisted opt-in flag; the browser permission itself lives in the user agent. */
const STORAGE_KEY = 'rd.notifications'

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

export interface Notifications {
  supported: boolean
  enabled: Ref<boolean>
  permission: Ref<NotificationPermission>
  enable: () => Promise<boolean>
  disable: () => void
  notify: (title: string, body?: string) => void
}

/** Desktop notifications for finished transfers; opt-in is remembered per browser. */
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

  function notify(title: string, body?: string): void {
    if (!supported || !enabled.value || permission.value !== 'granted') return
    try {
      const notification = new Notification(title, body ? { body } : undefined)
      notification.onclick = () => {
        window.focus()
        notification.close()
      }
    } catch {
      // some browsers only allow notifications from a service worker
    }
  }

  return { supported, enabled, permission, enable, disable, notify }
}
