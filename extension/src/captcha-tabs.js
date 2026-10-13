// The tabs, notifications and origin grants the widget captcha flow holds (RD-108-02), and how
// each is let go again; kept apart from the state machine in captcha.js (RD-1280-08) so each
// file stays readable.

import { STATE_KEYS } from './captcha-state.js'

const [TABS_KEY, GRANTS_KEY] = STATE_KEYS

/** Builds the helpers over the browser `api` and the session state's `read` and `mutate`. */
export function createCaptchaTabs(api, { read, mutate }) {
  /**
   * Drops the origin grant — unless another tracked tab or pending grant still needs it (two
   * widgets from one hoster). Callers remove and write their own entry first.
   */
  async function release(origin) {
    if (!origin) return
    const tabs = await read(TABS_KEY, {})
    const grants = await read(GRANTS_KEY, {})
    const stillNeeded =
      Object.values(tabs).some((entry) => entry.origin === origin) || Object.values(grants).includes(origin)
    if (stillNeeded) return
    try {
      await api.permissions.remove({ origins: [origin] })
    } catch {
      // Chrome refuses to drop an origin another grant still covers; harmless.
    }
  }

  async function clearNotification(notificationId) {
    try {
      await api.notifications?.clear?.(notificationId)
    } catch {
      // already closed by the person
    }
  }

  async function closeTab(tabId) {
    try {
      await api.tabs.remove(Number(tabId))
    } catch {
      // already closed by the person
    }
  }

  async function forget(tabId) {
    return mutate(TABS_KEY, {}, (tabs) => {
      const entry = tabs[String(tabId)]
      if (!entry) return null
      delete tabs[String(tabId)]
      return entry
    })
  }

  async function recordGrant(id, origin) {
    await mutate(GRANTS_KEY, {}, (grants) => { grants[id] = origin })
  }

  async function dropGrant(id) {
    await mutate(GRANTS_KEY, {}, (grants) => { delete grants[id] })
  }

  return { release, clearNotification, closeTab, forget, recordGrant, dropGrant }
}
