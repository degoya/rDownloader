// The half of the captcha flow that is about the hoster's page itself (RD-108-02).
//
// Split out of `captcha.js` the way `captcha-api.js` was: that file is the state machine in the
// background, this one is what the page means — which field each widget vendor fills, which
// origin pattern names exactly that page, and the reader that runs inside it. `captcha.js`
// re-exports everything here, so nothing that imports the feature has to know about the split.

/**
 * How long the reader keeps looking at an unanswered widget before it gives up, in milliseconds.
 *
 * A poll that never ends is a poll that runs until the tab closes. Ten minutes outlives any
 * captcha the server is still waiting for, and a widget answered after it is caught by the next
 * `complete` on the tab anyway.
 */
export const HARVEST_DEADLINE_MS = 600_000

/**
 * How long a page may go without showing any widget before the reader reports it missing.
 *
 * The case RD-120-45 was reported for: the service met a Turnstile on the hoster's sign-in page,
 * fetched as a guest, while the person's browser is signed in there and is sent straight past the
 * form. Nothing on that page can ever be answered, and the reader used to watch it for the full
 * deadline while the sign-in waited in silence. Widget scripts render within a few seconds of the
 * page's `complete`; a page that shows no trace of one after this long has none.
 */
export const NO_WIDGET_AFTER_MS = 15_000

/**
 * What counts as "a widget is on this page", whether answered or not: any vendor's answer field,
 * its container, or its frame. Read, never written, like the answer fields.
 */
export const WIDGET_MARKERS = [
  'input[name="cf-turnstile-response"]',
  'textarea[name="g-recaptcha-response"]',
  'textarea[name="h-captcha-response"]',
  '.cf-turnstile',
  '.g-recaptcha',
  '.h-captcha',
  '[data-sitekey]',
  'iframe[src*="challenges.cloudflare.com"]',
  'iframe[src*="/recaptcha/"]',
  'iframe[src*="hcaptcha.com"]'
].join(', ')

/** The answer field each widget vendor fills on the hoster's page. Read, never written. */
export const ANSWER_FIELDS = {
  turnstile: 'input[name="cf-turnstile-response"]',
  recaptcha_v2: 'textarea[name="g-recaptcha-response"]',
  h_captcha: 'textarea[name="h-captcha-response"]'
}

/** Selector for a widget kind; an unknown kind watches all three rather than none. */
export function answerFieldFor(kind) {
  return ANSWER_FIELDS[kind] ?? Object.values(ANSWER_FIELDS).join(', ')
}

/** `<scheme>://<host>/*` for exactly the page's origin: not its subdomains, never all sites. */
export function pageOriginPattern(pageUrl) {
  try {
    const url = new URL(String(pageUrl ?? ''))
    if (!/^https?:$/.test(url.protocol)) return null
    return `${url.protocol}//${url.host}/*`
  } catch {
    return null
  }
}

/** Host name of the page, for the texts the person reads. */
export function hostOf(pageUrl) {
  try {
    return new URL(String(pageUrl ?? '')).hostname
  } catch {
    return ''
  }
}

/**
 * Runs inside the hoster's page. Self-contained on purpose: `scripting.executeScript`
 * serialises the function, so it may use nothing from this module. It reads one field until
 * the widget fills it, sends the token exactly once, and changes nothing on the page.
 *
 * Two things it did not do (RD-109-23, finding 3). The 500-millisecond poll was cleared only
 * when a token was found, so an unanswered widget left it running until the tab closed. And
 * `onTabUpdated` injects on every `complete` — deliberately, so a navigation is followed — which
 * started a second poll in the same document and could send the same token twice; the second
 * send was answered with `untracked`, because the first had already taken the entry.
 *
 * The marker lives on the injection's own global, which is the isolated world of this document
 * and this extension: a real navigation builds a new one, a second injection into the same
 * document finds the old one. Returns whether this call took the job on, for the tests.
 */
// The default repeats `HARVEST_DEADLINE_MS` as a literal deliberately: a default argument is
// evaluated in the page, where nothing from this module exists. The call site passes the
// constant, and this is only the value a hand call would get.
//
// `missing` — `{ messageType, afterMs, markers }` — makes it also say when the page shows no
// widget at all (RD-120-45): once `afterMs` has passed without any of `markers` ever matching,
// it sends `{ type: missing.messageType, id }` instead of a token, once, and stops. A widget seen
// once counts as present for good, so a widget that resets or re-renders is never reported.
export function harvestAnswer(captchaId, selector, messageType, deadlineMs = 600_000, missing = null) {
  const runtime = (globalThis.browser ?? globalThis.chrome)?.runtime
  if (!runtime) return false
  const marker = '__rdownloaderCaptchaHarvester'
  if (globalThis[marker]) return false
  globalThis[marker] = captchaId
  let sent = false
  let seen = false
  let timer = null
  const started = Date.now()
  const stop = () => {
    if (timer === null) return
    clearInterval(timer)
    timer = null
  }
  const look = () => {
    if (sent) return stop()
    const field = document.querySelector(selector)
    const token = typeof field?.value === 'string' ? field.value.trim() : ''
    if (!token) {
      if (missing && !seen) seen = Boolean(field || document.querySelector(missing.markers))
      if (missing && !seen && Date.now() - started >= missing.afterMs) {
        sent = true
        stop()
        try {
          runtime.sendMessage({ type: missing.messageType, id: captchaId })
        } catch {
          // The extension was reloaded under the page; the captcha runs into its timeout.
        }
        return
      }
      if (Date.now() - started >= deadlineMs) stop()
      return
    }
    sent = true
    stop()
    try {
      runtime.sendMessage({ type: messageType, id: captchaId, token })
    } catch {
      // The extension was reloaded under the page; the tab is closed from the other side.
    }
  }
  timer = setInterval(look, 500)
  look()
  return true
}
