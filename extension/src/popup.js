import { api, loadConfig, message } from './browser.js'
import { DECLINE_MESSAGE, createCaptchaAnswerer, hostOf } from './captcha.js'
import { allowSite, revokeSite, siteConsented, siteHost } from './files.js'
import { createHandover, scopeHost, scopeOrigin } from './handover.js'

/**
 * What the clipboard button should do with what the clipboard gave it.
 *
 * Returns `{ status }` when there is nothing to send, `{ text }` when there is. Split out of
 * the click handler because a page without a DOM cannot be tested, and these two branches —
 * the refused clipboard and the clipboard without links — never were (RD-109-26).
 */
/**
 * The version the popup is actually running, read from the manifest rather than restated.
 *
 * Asked for from use: a temporary install and a signed one look identical, and after RD-109-17
 * the manifest finally carries the release version — so the one place a person looks is the
 * one place it can be shown without a second source to keep in step (RD-109-46).
 */
export function versionLine(getManifest) {
  try {
    const version = getManifest()?.version
    return version ? `v${version}` : ''
  } catch {
    return ''
  }
}

export async function clipboardLinks(readText) {
  let text = ''
  try {
    text = await readText()
  } catch {
    return { status: 'clipboardDenied' }
  }
  if (!/https?:\/\//i.test(String(text))) return { status: 'clipboardNoLinks' }
  return { text: String(text) }
}

/**
 * Whether a failed widget listing is worth a line of status.
 *
 * A 401 is the unpaired state, which the options page already explains; repeating it in the
 * popup on every open would be noise. `unconfigured` is the same state seen from the background,
 * which is where the listing comes from now (RD-109-23).
 */
export function captchaListFailure(result) {
  if (result?.ok) return null
  if (result?.status === 401 || result?.code === 'unconfigured') return null
  return { key: 'captchaListFailed', substitutions: [result?.message || result?.code || ''] }
}

/** The address of the tab the popup was opened over, or null when it has none to send. */
export function sendablePageUrl(tab) {
  return /^https?:/.test(String(tab?.url ?? '')) ? tab.url : null
}

/**
 * What the popup says after a handover click, as a message key and its substitutions.
 *
 * Split out of the click handler for the same reason as `clipboardLinks`: a page without a DOM
 * cannot be tested, and each of these outcomes needs its own words (RD-120-45).
 */
export function handoverOutcome(result, host) {
  if (result?.ok) return { key: 'handoverDone', substitutions: [host], ok: true }
  if (result?.code === 'denied') return { key: 'handoverDenied', substitutions: [host], ok: false }
  if (result?.code === 'empty') return { key: 'handoverEmpty', substitutions: [host], ok: false }
  return { key: 'handoverFailed', substitutions: [host, result?.message || result?.code || ''], ok: false }
}

/**
 * Whether a failed listing of session requests is worth a line of status. Unpaired is explained
 * elsewhere, as for captchas; a 404 is a rDownloader older than the feature, which has nothing
 * to list rather than something broken.
 */
export function handoverListFailure(result) {
  if (result?.status === 404) return null
  const failure = captchaListFailure(result)
  return failure ? { ...failure, key: 'handoverListFailed' } : null
}

/**
 * What the popup says after a click on the site's file consent (RD-130-16), as a message key and
 * its substitutions — split out for the same reason as the outcomes above.
 */
export function filesOutcome(result) {
  if (result?.ok) return { key: result.revoked ? 'filesRevoked' : 'filesAllowed', substitutions: [result.host], ok: true }
  return { key: 'filesDenied', substitutions: [result?.host ?? ''], ok: false }
}

function setStatus(text, ok) {
  const status = document.getElementById('status')
  status.textContent = text
  status.className = ok ? 'ok' : 'error'
}

async function sendToBackground(payload) {
  const response = await api.runtime.sendMessage({ type: 'rdownloader:send', ...payload })
  setStatus(response?.ok ? message('sentFromPopup') : message('sendFailed', [response?.message ?? '']), Boolean(response?.ok))
}

function captchaButton(label, onClick, secondary = false) {
  const button = document.createElement('button')
  button.textContent = label
  if (secondary) button.className = 'secondary'
  button.addEventListener('click', onClick)
  return button
}

/**
 * Lists the waiting widgets with an answer and a decline button each; hidden when none wait.
 *
 * The listing is the background's own poll (RD-109-23): it called `listWidgets` directly, so
 * opening the popup announced nothing, left stale notification ids in place and never touched
 * the badge — the list and the number beside it could disagree indefinitely.
 */
async function renderCaptchas(captchas) {
  const section = document.getElementById('captchas')
  const list = document.getElementById('captcha-list')
  const result = await captchas.requestPoll()
  if (!result.ok) {
    section.hidden = true
    const failure = captchaListFailure(result)
    if (failure) setStatus(message(failure.key, failure.substitutions), false)
    return
  }
  list.replaceChildren()
  for (const widget of result.widgets) {
    const host = hostOf(widget.page_url)
    const item = document.createElement('div')
    item.className = 'captcha'
    const label = document.createElement('span')
    label.className = 'host'
    label.textContent = message('captchaFor', [host])
    const row = document.createElement('div')
    row.className = 'row'
    row.append(
      captchaButton(message('captchaAnswer'), async () => {
        const opened = await captchas.open(widget)
        if (opened.ok) setStatus(message('captchaOpened'), true)
        else if (opened.code === 'denied') setStatus(message('captchaPermissionDenied', [host]), false)
        else setStatus(message('sendFailed', [opened.code ?? '']), false)
      }),
      captchaButton(message('captchaDecline'), async () => {
        const response = await api.runtime.sendMessage({ type: DECLINE_MESSAGE, id: widget.id })
        // "Declined" only when the server said so; otherwise what it said instead (RD-109-23).
        setStatus(
          response?.ok ? message('captchaSkipped', [host]) : message('captchaSkipFailed', [host, response?.message || response?.code || '']),
          Boolean(response?.ok)
        )
        await renderCaptchas(captchas)
      }, true)
    )
    item.append(label, row)
    list.append(item)
  }
  section.hidden = result.widgets.length === 0
}

/**
 * Lists the sessions a person asked for in the web interface, each with its own consent and
 * decline button; hidden when none wait (RD-120-45).
 *
 * Whether the origin is already granted is asked here, while drawing, because the click itself
 * may await nothing before the permission request. The answer tells the background whether the
 * origin is to be given back afterwards.
 */
async function renderHandovers(handovers) {
  const section = document.getElementById('handovers')
  const list = document.getElementById('handover-list')
  const result = await handovers.requestPoll()
  if (!result.ok) {
    section.hidden = true
    const failure = handoverListFailure(result)
    if (failure) setStatus(message(failure.key, failure.substitutions), false)
    return
  }
  list.replaceChildren()
  for (const handover of result.handovers) {
    const host = scopeHost(handover.scope)
    let hadOrigin = false
    try {
      hadOrigin = await api.permissions.contains({ origins: [scopeOrigin(handover.scope)] })
    } catch {
      hadOrigin = false
    }
    const item = document.createElement('div')
    item.className = 'handover'
    const label = document.createElement('span')
    label.className = 'host'
    label.textContent = message('handoverFor', [host, handover.account_label ?? ''])
    const note = document.createElement('span')
    note.className = 'note'
    note.textContent = message('handoverNote', [host])
    const row = document.createElement('div')
    row.className = 'row'
    row.append(
      captchaButton(message('handoverAllow'), async () => {
        const outcome = handoverOutcome(await handovers.consent(handover, { hadOrigin }), host)
        setStatus(message(outcome.key, outcome.substitutions), outcome.ok)
        await renderHandovers(handovers)
      }),
      captchaButton(message('handoverDecline'), async () => {
        const response = await handovers.requestDecline(handover.id)
        setStatus(
          response?.ok ? message('handoverDeclined', [host]) : message('handoverFailed', [host, response?.message || response?.code || '']),
          Boolean(response?.ok)
        )
        await renderHandovers(handovers)
      }, true)
    )
    item.append(label, note, row)
    list.append(item)
  }
  section.hidden = result.handovers.length === 0
}

/**
 * The site's file consent: one button that allows or revokes it for the tab's host; hidden on a
 * page that is not a web address (RD-130-16). Whether the site is allowed is asked while
 * drawing, because the allow click may await nothing before the permission request.
 */
async function renderFiles(tab) {
  const section = document.getElementById('files')
  const button = document.getElementById('files-toggle')
  const url = sendablePageUrl(tab)
  const host = siteHost(url)
  if (!host) {
    section.hidden = true
    return
  }
  const allowed = await siteConsented(api, url)
  button.textContent = message(allowed ? 'filesRevoke' : 'filesAllow', [host])
  document.getElementById('files-note').textContent = allowed ? '' : message('filesNote', [host])
  button.onclick = async () => {
    const result = allowed ? { ...(await revokeSite(api, host)), revoked: true } : await allowSite(api, url)
    const outcome = filesOutcome(result)
    setStatus(message(outcome.key, outcome.substitutions), outcome.ok)
    await renderFiles(tab)
  }
  section.hidden = false
}

async function init() {
  // The popup only opens the hoster page: the permission prompt needs the click as its user
  // gesture, which a message to the background would not carry. Everything after the tab exists
  // happens in the background (RD-108-02).
  const captchas = createCaptchaAnswerer({ api, loadConfig, message, notify: async () => null })
  for (const element of document.querySelectorAll('[data-i18n]')) {
    element.textContent = message(element.dataset.i18n)
  }
  void renderCaptchas(captchas)
  void renderHandovers(createHandover({ api, loadConfig, message, notify: async () => null }))
  void api.tabs.query({ active: true, currentWindow: true }).then(([tab]) => renderFiles(tab))
  document.getElementById('page').addEventListener('click', async () => {
    const [tab] = await api.tabs.query({ active: true, currentWindow: true })
    const url = sendablePageUrl(tab)
    if (!url) {
      setStatus(message('noPageUrl'), false)
      return
    }
    await sendToBackground({ text: url, packageName: tab.title })
  })
  document.getElementById('clipboard').addEventListener('click', async () => {
    const outcome = await clipboardLinks(() => navigator.clipboard.readText())
    if (outcome.status) {
      setStatus(message(outcome.status), false)
      return
    }
    await sendToBackground({ text: outcome.text, sourceLabel: 'Clipboard' })
  })
  document.getElementById('options').addEventListener('click', () => api.runtime.openOptionsPage())
  document.getElementById('version').textContent = versionLine(() => api.runtime.getManifest())
}

// The popup page is the only consumer; a test imports the exports above and never runs this.
if (globalThis.document) void init()
