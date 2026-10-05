// The widget captcha flow's session state and the badge it paints (RD-108-02), kept apart from
// the state machine in captcha.js so each file stays readable.

/** The session-state keys this module owns. Named once, so the legacy cleanup cannot miss one. */
export const STATE_KEYS = [
  'captchaTabs',
  'captchaGrants',
  'captchaAnnounced',
  'captchaNotifications',
  'captchaBadge',
  'captchaSendFailure'
]

const [, , , , BADGE_KEY, SEND_FAILURE_KEY] = STATE_KEYS

/** Builds the state store on `api.storage.session`, or on this worker's memory without one. */
export function createCaptchaState(api) {
  /**
   * Captcha state is session state, and where the browser has no session area it stays in this
   * worker's memory rather than falling back to `storage.local` (RD-109-23, finding 4).
   *
   * Tab ids, origin grants, announcements and notification ids outlive nothing: written to disk
   * they survive a browser restart, and `pollOnce` then closes tabs by ids that now belong to
   * somebody else's windows — the extension closing a window it never opened. Losing the state
   * with the worker is the correct loss: the server still knows which captchas wait, the next
   * poll finds them again, and the worst case is a hoster tab the person closes by hand.
   */
  const memory = new Map()
  const store = api.storage?.session ?? {
    // Copies in and out, as the real area does: a caller must not be able to change what is
    // stored by holding on to what it read.
    get: async (defaults) =>
      Object.fromEntries(Object.keys(defaults).map(
        (key) => [key, memory.has(key) ? structuredClone(memory.get(key)) : defaults[key]]
      )),
    set: async (values) => { for (const [key, value] of Object.entries(values)) memory.set(key, structuredClone(value)) }
  }

  async function read(key, fallback) {
    try {
      const stored = await store.get({ [key]: fallback })
      return stored?.[key] ?? fallback
    } catch {
      return fallback
    }
  }

  async function write(key, value) {
    try {
      await store.set({ [key]: value })
    } catch {
      // Without storage the flow still works within one worker lifetime.
    }
  }

  /**
   * Every change to a stored map goes through here, one at a time.
   *
   * `openTab` used to read the tab map, await `tabs.create` — the slow part — and write the
   * whole map back afterwards. A `forget` that ran in that window was overwritten, leaving an
   * entry for a tab that was already closed; the entry carries the origin, `release` reads it as
   * "still needed", and the hoster permission is never given back (RD-109-23, finding 2).
   * `recordGrant`, `dropGrant` and `forget` shared the pattern. Here the read and the write have
   * nothing but the caller's own synchronous change between them, and a second change waits.
   */
  let mutations = Promise.resolve()
  function mutate(key, fallback, change) {
    const done = mutations.then(async () => {
      const current = await read(key, fallback)
      const result = change(current)
      await write(key, current)
      return result
    })
    mutations = done.then(() => undefined, () => undefined)
    return done
  }

  /**
   * The badge has exactly one owner, and it is this module (RD-109-24).
   *
   * The background used to write the badge directly on a link send while this module kept its
   * own count in session storage. A successful send then cleared the number of waiting captchas,
   * and because `badge` returned early when the count had not changed, the next poll left it
   * cleared — the badge stayed blank until the number itself moved. Both inputs now go through
   * `paint`, which composes them: a failed send outranks a count, because it is the one thing
   * that needs an answer now.
   */
  async function paint() {
    const count = await read(BADGE_KEY, 0)
    const failed = await read(SEND_FAILURE_KEY, false)
    const text = failed ? '!' : count > 0 ? String(count) : ''
    try {
      await api.action?.setBadgeText?.({ text })
      if (text) await api.action?.setBadgeBackgroundColor?.({ color: failed ? '#e9524b' : '#14b8b0' })
    } catch {
      // no action badge in this browser
    }
  }

  async function badge(count) {
    if ((await read(BADGE_KEY, 0)) === count) return
    await write(BADGE_KEY, count)
    await paint()
  }

  /** The background reports the outcome of a link send here instead of painting itself. */
  async function setSendFailure(failed) {
    const value = Boolean(failed)
    if ((await read(SEND_FAILURE_KEY, false)) === value) return
    await write(SEND_FAILURE_KEY, value)
    await paint()
  }

  /**
   * Drops captcha state an earlier version wrote to `storage.local` where this browser has no
   * session area. Nothing reads those keys any more; leaving them would keep tab ids from a
   * previous browser session on the person's disk for no purpose.
   */
  async function discardPersistedState() {
    try {
      await api.storage?.local?.remove?.(STATE_KEYS)
    } catch {
      // no local area, or it refused; nothing here is read either way
    }
  }

  return { read, write, mutate, badge, setSendFailure, discardPersistedState }
}
