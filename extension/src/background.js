import { ping, submitCapture, submitLinks } from './api.js'
import { api, loadConfig, message } from './browser.js'
import { MESSAGE_TYPES as CAPTCHA_MESSAGES, POLL_ALARM, createCaptchaAnswerer } from './captcha.js'
import { createInterceptor } from './downloads.js'
import { FILTERED_TYPES, createFileHandover } from './files.js'
import { MESSAGE_TYPES as HANDOVER_MESSAGES, createHandover } from './handover.js'
import { createSessionSharer } from './session.js'

const MENU_LINK = 'rdownloader-link'
const MENU_PAGE = 'rdownloader-page'
const MENU_SELECTION = 'rdownloader-selection'
const MENU_SESSION = 'rdownloader-session'

function createMenus() {
  api.contextMenus.removeAll(() => {
    api.contextMenus.create({ id: MENU_LINK, title: message('menuLink'), contexts: ['link'] })
    api.contextMenus.create({ id: MENU_PAGE, title: message('menuPage'), contexts: ['page'] })
    api.contextMenus.create({ id: MENU_SELECTION, title: message('menuSelection'), contexts: ['selection'] })
    // Sharing a session is a deliberate act on the page the user is looking at; the
    // permission prompt the browser shows needs this click as its user gesture.
    api.contextMenus.create({ id: MENU_SESSION, title: message('menuSession'), contexts: ['page'] })
  })
}

api.runtime.onInstalled.addListener(createMenus)
api.runtime.onStartup?.addListener(createMenus)

async function notify(title, body) {
  try {
    return await api.notifications.create({
      type: 'basic',
      iconUrl: api.runtime.getURL('icons/icon128.png'),
      title,
      message: body
    })
  } catch {
    // notifications may be unavailable; the badge still reflects the outcome
    return null
  }
}

/**
 * Hands a listener's promise off without letting a throw disappear.
 *
 * Every dispatch here used to be a bare `void p`. A throw inside any of them became an unhandled
 * rejection in the service-worker log, with nothing naming which handler it came from, and in
 * the notification chain it also swallowed the fallback (RD-109-24).
 */
export function dispatch(where, work) {
  Promise.resolve()
    .then(work)
    .catch((error) => {
      console.error(`rdownloader: ${where} failed`, error)
    })
}

/**
 * Offers a notification to each possible owner in turn, until one claims it.
 *
 * A throw from the first owner used to swallow the second: the click on "keep in browser" then
 * did nothing at all, because the captcha side had rejected before the fallback was reached
 * (RD-109-24). A broken owner is logged and the next one is still asked.
 */
export async function offerNotification(notificationId, owners) {
  for (const owner of owners) {
    try {
      if (await owner(notificationId)) return true
    } catch (error) {
      console.error('rdownloader: a notification owner failed', error)
    }
  }
  return false
}

async function send({ text, packageName, sourceLabel }) {
  const config = await loadConfig()
  if (!config.token) {
    await captchas.setSendFailure(true)
    await notify(message('extName'), message('notConfigured'))
    api.runtime.openOptionsPage()
    return
  }
  const result = await submitLinks(config, { text, packageName, sourceLabel })
  // The badge belongs to the captcha answerer, which composes this with the number of waiting
  // widgets. Painting it here cleared that number and left it cleared (RD-109-24).
  await captchas.setSendFailure(!result.ok)
  if (result.ok) {
    await notify(message('extName'), message('sentLinks', [String(result.links)]))
  } else {
    const reason = result.status === 401 ? message('errorUnauthorized') : result.message
    await notify(message('extName'), message('sendFailed', [reason]))
  }
}

api.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === MENU_LINK && info.linkUrl) {
    dispatch('contextMenus.onClicked', () => send({ text: info.linkUrl, sourceLabel: 'Browser' }))
  } else if (info.menuItemId === MENU_PAGE && (info.pageUrl || tab?.url)) {
    dispatch('contextMenus.onClicked', () => send({ text: info.pageUrl || tab.url, packageName: tab?.title, sourceLabel: 'Browser' }))
  } else if (info.menuItemId === MENU_SELECTION && info.selectionText) {
    dispatch('contextMenus.onClicked', () => send({ text: info.selectionText, sourceLabel: 'Browser' }))
  } else if (info.menuItemId === MENU_SESSION && (info.pageUrl || tab?.url)) {
    dispatch('contextMenus.onClicked', () => sessions.share(info.pageUrl || tab.url, { name: tab?.title }))
  }
})

const sessions = createSessionSharer({
  api,
  loadConfig,
  message,
  notify: (body) => notify(message('extName'), body)
})

// NZB, torrent and ZIP files from a site the person allowed (RD-130-16): the bytes Firefox copied,
// or the address with that host's cookies.
const files = createFileHandover({
  api,
  loadConfig,
  message,
  notify: (body) => notify(message('extName'), body),
  userAgent: globalThis.navigator?.userAgent ?? ''
})

const interceptor = createInterceptor({
  api,
  loadConfig,
  submit: submitCapture,
  ping,
  message,
  userAgent: globalThis.navigator?.userAgent ?? '',
  ownExtensionId: api.runtime.id,
  // Whether this browser lets us watch requests at all. Without it "we watched and found
  // nothing" cannot be told from "we could not watch", and only the first may refuse a
  // download (RD-109-18).
  observing: Boolean(api.webRequest),
  files
})

// Widget captchas answered in this browser (RD-108-02). Polling runs on an alarm so a
// service-worker restart does not stop it; the tab map lives in session storage for the same
// reason. The popup only asks for the permission (the prompt needs its click); this side
// opens the hoster page, injects the reader, takes the token back and declines on a closed tab.
const captchas = createCaptchaAnswerer({
  api,
  loadConfig,
  message,
  notify: (body) => notify(message('extName'), body)
})

// Sessions a person asked for at an account in the web interface (RD-120-45). Polled on the
// captcha alarm, which runs exactly while the extension is paired; the popup asks for consent.
const handovers = createHandover({
  api,
  loadConfig,
  message,
  notify: (body) => notify(message('extName'), body)
})

api.runtime.onInstalled.addListener(() => dispatch('captchas.schedule', () => captchas.schedule()))
api.runtime.onStartup?.addListener(() => dispatch('captchas.schedule', () => captchas.schedule()))
api.alarms?.onAlarm?.addListener((alarm) => dispatch('captchas.onAlarm', () => captchas.onAlarm(alarm)))
api.alarms?.onAlarm?.addListener((alarm) =>
  dispatch('handovers.onAlarm', () => (alarm?.name === POLL_ALARM ? handovers.poll() : null)))
api.tabs?.onUpdated?.addListener((tabId, changeInfo) => dispatch('captchas.onTabUpdated', () => captchas.onTabUpdated(tabId, changeInfo)))
api.tabs?.onRemoved?.addListener((tabId) => dispatch('captchas.onTabRemoved', () => captchas.onTabRemoved(tabId)))
// Pairing or unpairing in the options page starts or stops the polling.
api.storage?.onChanged?.addListener((changes, area) => {
  if (area === 'local' && changes.token) dispatch('captchas.schedule', () => captchas.schedule())
})

api.downloads?.onCreated.addListener((item) => dispatch('interceptor.onDownloadCreated', () => interceptor.onDownloadCreated(item)))
api.notifications?.onButtonClicked?.addListener((id, index) =>
  dispatch('interceptor.onNotificationButtonClicked', () => interceptor.onNotificationButtonClicked(id, index)))
// A notification belongs either to a waiting captcha or to a paused download; the captcha side
// is asked first and says whether it was its own.
api.notifications?.onClicked?.addListener((id) =>
  dispatch('notifications.onClicked', () =>
    offerNotification(id, [
      (notificationId) => captchas.onNotificationClicked(notificationId),
      (notificationId) => handovers.onNotificationClicked(notificationId),
      (notificationId) => interceptor.onNotificationClicked(notificationId)
    ])))

const REQUEST_FILTER = { urls: ['<all_urls>'], types: ['main_frame', 'sub_frame', 'xmlhttprequest', 'other'] }

/** Registers a webRequest listener, retrying without `extraHeaders` where it is unsupported (Firefox). */
function addRequestListener(event, handler, extraInfoSpec) {
  try {
    event.addListener(handler, REQUEST_FILTER, [...extraInfoSpec, 'extraHeaders'])
  } catch {
    event.addListener(handler, REQUEST_FILTER, extraInfoSpec)
  }
}

// These three listeners only observe: which method a request used, which of a small
// allowlist of headers it carried, and the Content-Disposition the server answered with. No
// listener asks for `requestBody`, and none ever will — the POST-body path was removed with
// RD-109-20, because no part of this extension ever requested the broad host access it needed,
// so in a default install it never once ran. A download the page started with a POST is kept by
// the browser and explained instead of being replayed as a GET.
if (api.webRequest) {
  addRequestListener(api.webRequest.onSendHeaders, interceptor.onSendHeaders, ['requestHeaders'])
  addRequestListener(api.webRequest.onHeadersReceived, interceptor.onHeadersReceived, ['responseHeaders'])
  // `extraHeaders` is not valid for onBeforeRequest, so this one is registered directly.
  api.webRequest.onBeforeRequest.addListener(interceptor.onBeforeRequest, REQUEST_FILTER)
}

// The one blocking listener, and only where `filterResponseData` exists (Firefox, which the build
// grants `webRequestBlocking` and `webRequestFilterResponse`). It answers every response at once
// except an NZB, torrent or ZIP navigation from an allowed site, which waits only until the copy
// is in place (RD-130-16). Chrome has no such API and never registers it.
// A browser that refuses `blocking` throws here; that costs the copy, never the listeners below.
if (typeof api.webRequest?.filterResponseData === 'function') {
  try {
    api.webRequest.onHeadersReceived.addListener(
      files.onHeadersReceived,
      { urls: ['<all_urls>'], types: FILTERED_TYPES },
      ['blocking', 'responseHeaders']
    )
  } catch (error) {
    console.error('rdownloader: the response copy is unavailable', error)
  }
}

api.runtime.onMessage.addListener((request, sender, sendResponse) => {
  if (request?.type === 'rdownloader:send') {
    send({ text: request.text, packageName: request.packageName, sourceLabel: request.sourceLabel ?? 'Browser' })
      .then(() => sendResponse({ ok: true }))
      .catch((error) => sendResponse({ ok: false, message: String(error) }))
    return true
  }
  // The popup announces a grant, asks for the tab, polls and declines here, because it may be
  // gone the moment the tab becomes active; the hoster tab sends the token here for the same
  // reason. Declining used to be a branch of its own that skipped the sender check the other
  // messages get; it goes through the one handler now (RD-109-23).
  if (CAPTCHA_MESSAGES.includes(request?.type)) {
    captchas.onMessage(request, sender)
      .then((result) => sendResponse(result))
      .catch((error) => sendResponse({ ok: false, message: String(error) }))
    return true
  }
  // The popup's consent, delivery, decline and listing of session handovers (RD-120-45). The
  // handler checks the sender itself: only our own pages get an answer.
  if (HANDOVER_MESSAGES.includes(request?.type)) {
    handovers.onMessage(request, sender)
      .then((result) => sendResponse(result))
      .catch((error) => sendResponse({ ok: false, message: String(error) }))
    return true
  }
  return false
})
