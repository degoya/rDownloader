// Files a page builds itself, handed over from a site the person allowed (RD-1460-02).
//
// Some sites never serve a container from an address: the page puts it together in script
// (`new Blob`, `URL.createObjectURL`) and starts a download of the `blob:` address, which it
// revokes the moment the click is through. The download interceptor only takes `http(s)`
// downloads, and nothing outside the page can read a `blob:` address anyway, so such a file
// always stayed in the browser.
//
// On an allowed site two content scripts take it instead:
//
// - `page-files-main.js` runs in the page's own world, remembers the blobs the page makes, and
//   holds back the download of a browser-only file (an NZB, torrent, ZIP or link container, by
//   name or by the blob's type).
// - `page-files-relay.js` runs in the extension's world on the same page and carries the bytes
//   here, and the answer back.
//
// Here the same rules hold as for every other hand-over of a browser-only file: a token, "take
// over browser downloads" on, the person's type filter, and the site's consent — read from the
// browser's own `sender.url`, never from the page. Taken, the page's download never happens;
// refused or not ours, the page script starts it after all, so the browser gets the file.

import { captureFilter, passesTypeFilter } from './file-types.js'
import { keptNotice, loadSites, siteConsented, sitePattern, submitFileBytes } from './files.js'
import { baseName, handOverName, staysInBrowser } from './intercept.js'

/** The runtime message the relay sends, and the `type` of the page's own messages. */
export const PAGE_FILE_MESSAGE = 'rdownloader:page-file'

/**
 * The most a page's file may hold. Below `MAX_FILE_BYTES`: the bytes travel as base64 in a
 * runtime message, and Chrome refuses a message over 64 MiB. Containers, NZBs and torrents are a
 * few kilobytes; `page-files-main.js` carries the same number.
 */
export const PAGE_FILE_MAX_BYTES = 16 * 1024 * 1024

/** The ids of the two registered scripts. */
export const PAGE_SCRIPT_IDS = ['rdownloader-page-files-main', 'rdownloader-page-files-relay']

/** The registrations for these hosts: the page script in the page's world, the relay beside it. */
export function pageScripts(hosts) {
  const matches = hosts.map(sitePattern)
  const common = { matches, runAt: 'document_start', allFrames: false, persistAcrossSessions: false }
  return [
    { id: PAGE_SCRIPT_IDS[0], js: ['src/page-files-main.js'], world: 'MAIN', ...common },
    { id: PAGE_SCRIPT_IDS[1], js: ['src/page-files-relay.js'], ...common }
  ]
}

/** Bytes from base64, or null for anything that is not base64, empty or too large. */
export function decodeBytes(text) {
  if (typeof text !== 'string' || (text.length / 4) * 3 > PAGE_FILE_MAX_BYTES + 3) return null
  try {
    const binary = atob(text)
    if (binary.length === 0 || binary.length > PAGE_FILE_MAX_BYTES) return null
    const bytes = new Uint8Array(binary.length)
    for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index)
    return bytes
  } catch {
    return null
  }
}

/** Builds the page-file hand-over; everything browser-specific is injected for the tests. */
export function createPageFiles({
  api,
  loadConfig,
  message,
  notify,
  submitBytes = submitFileBytes,
  // Why a file stayed in the browser, in the extension's console: nothing else says it (RD-1470-03).
  log = (reason) => console.info(`rDownloader: a page's file stays in the browser (${reason})`)
}) {
  let pending = Promise.resolve()

  /** Registers the scripts for exactly the allowed sites whose grant still stands. */
  async function register() {
    const scripting = api.scripting
    if (typeof scripting?.registerContentScripts !== 'function') return
    const hosts = []
    for (const host of await loadSites(api)) {
      try {
        if (await api.permissions.contains({ origins: [sitePattern(host)] })) hosts.push(host)
      } catch {
        // a host the browser cannot answer for is left out
      }
    }
    const registered = (await scripting.getRegisteredContentScripts({ ids: PAGE_SCRIPT_IDS })) ?? []
    const ids = registered.map((script) => script.id)
    if (ids.length) await scripting.unregisterContentScripts({ ids })
    if (hosts.length) await scripting.registerContentScripts(pageScripts(hosts))
  }

  /** One registration at a time: two at once would both register the same ids. */
  function sync() {
    pending = pending.then(register, register)
    return pending
  }

  /** The browser keeps the file; the reason goes to the console. */
  function kept(reason) {
    log(reason)
    return { taken: false }
  }

  async function onMessage(request, sender) {
    // Only this extension's content script in a tab, and its frame's address as the browser
    // reports it: the page decides neither.
    if (!sender || sender.id !== api.runtime.id || !sender.tab || typeof sender.url !== 'string') return kept('sender')
    const name = baseName(request?.name)
    const mime = String(request?.mime ?? '')
    const item = { mime, filename: name }
    if (staysInBrowser(item) !== 'file') return kept('not a browser-only file')
    const config = await loadConfig()
    if (!config?.token) return kept('no capture token')
    if (config.interceptDownloads === false) return kept('taking over browser downloads is off')
    if (!passesTypeFilter(item, null, captureFilter(config))) return kept('file type filter')
    if (!(await siteConsented(api, sender.url))) return kept('site not allowed')
    const bytes = decodeBytes(request?.data)
    if (!bytes) return kept('no bytes')
    const result = await submitBytes(config, { bytes: new Blob([bytes]), fileName: handOverName(name, mime) })
    if (result.ok) {
      await notify?.(message('fileHandedOver'))
      return { taken: true }
    }
    const notice = keptNotice(result, message)
    if (notice) await notify?.(notice)
    return kept(`refused: ${result.code ?? result.status}`)
  }

  return { sync, onMessage }
}
