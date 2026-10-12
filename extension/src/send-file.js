// "Send file to rDownloader…" (RD-1260-11): local NZBs, torrents and link containers, chosen or
// dropped on a page of the extension's own, handed to the service like a file the browser loaded.
//
// A page of its own and not the popup: Firefox closes the popup the moment a file picker opens,
// and the files it was to send go with it. Nothing here needs a desktop agent, so a person whose
// rDownloader runs on a NAS hands a file over from the browser alone.

import { api, loadConfig, message } from './browser.js'
import { submitFileBytes } from './files.js'
import { BROWSER_ONLY_EXTENSIONS } from './intercept.js'

/** What `capture/file` takes: NZBs, torrents, ZIPs of NZBs and the link containers. */
export const ACCEPTED_EXTENSIONS = BROWSER_ONLY_EXTENSIONS

/**
 * The stable code a file is refused under before anything is sent: its extension is none of
 * these. The capture agent refuses such a file under the same code (RD-1260-09).
 */
export const UNKNOWN_EXTENSION = 'capture.file_type_unsupported'

/** Whether the service takes a file of this name; the extension is the only thing the page knows. */
export function acceptedName(name) {
  const lower = String(name ?? '').trim().toLowerCase()
  return ACCEPTED_EXTENSIONS.some((extension) => lower.endsWith(extension) && lower.length > extension.length)
}

/**
 * Sends every file, one after the other, and answers one result per file in their order. A file
 * of an unknown type is refused here under `UNKNOWN_EXTENSION` and never reaches the service.
 */
export async function sendFiles(config, files, submit = submitFileBytes) {
  const results = []
  for (const file of files) {
    const name = String(file?.name ?? '')
    if (!acceptedName(name)) {
      results.push({ name, ok: false, code: UNKNOWN_EXTENSION, message: null })
      continue
    }
    const result = await submit(config, { bytes: file, fileName: name })
    results.push({ name, ok: result.ok, code: result.code, status: result.status, message: result.message })
  }
  return results
}

/**
 * The one notice for a whole selection, as message keys with their substitutions: how many went
 * over, which were of an unknown type, and which the service refused and why.
 */
export function summary(results, translate = message) {
  const parts = []
  const sent = results.filter((result) => result.ok)
  const unknown = results.filter((result) => result.code === UNKNOWN_EXTENSION)
  const refused = results.filter((result) => !result.ok && result.code !== UNKNOWN_EXTENSION)
  if (sent.length) parts.push({ key: 'sendFileSent', substitutions: [String(sent.length)] })
  if (unknown.length) parts.push({ key: 'sendFileUnknown', substitutions: [unknown.map((result) => result.name).join(', ')] })
  if (refused.length) {
    const reasons = refused.map((result) => `${result.name} (${reasonOf(result, translate)})`)
    parts.push({ key: 'sendFileRefused', substitutions: [reasons.join(', ')] })
  }
  return { parts, ok: refused.length === 0 && unknown.length === 0 && sent.length > 0 }
}

function reasonOf(result, translate) {
  if (result.status === 401) return translate('errorUnauthorized')
  return result.message || result.code || ''
}

function setStatus(text, ok) {
  const status = document.getElementById('status')
  status.textContent = text
  status.className = ok ? 'ok' : 'error'
}

async function notify(body) {
  try {
    await api.notifications.create({
      type: 'basic',
      iconUrl: api.runtime.getURL('icons/icon128.png'),
      title: message('extName'),
      message: body
    })
  } catch {
    // notifications may be unavailable; the page says the same
  }
}

async function handOver(files) {
  if (!files.length) return
  const config = await loadConfig()
  if (!config.token) {
    setStatus(message('notConfigured'), false)
    api.runtime.openOptionsPage()
    return
  }
  setStatus(message('sendFileSending'), true)
  const { parts, ok } = summary(await sendFiles(config, files))
  const text = parts.map((part) => message(part.key, part.substitutions)).join(' ')
  setStatus(text, ok)
  await notify(text)
}

function init() {
  for (const element of document.querySelectorAll('[data-i18n]')) {
    element.textContent = message(element.dataset.i18n)
  }
  document.title = message('sendFileTitle')
  document.getElementById('accepted').textContent = message('sendFileAccepted', [ACCEPTED_EXTENSIONS.join(' ')])
  const input = document.getElementById('files')
  input.accept = ACCEPTED_EXTENSIONS.join(',')
  input.addEventListener('change', () => {
    const files = Array.from(input.files ?? [])
    input.value = ''
    void handOver(files)
  })
  const area = document.getElementById('drop')
  area.addEventListener('dragover', (event) => {
    event.preventDefault()
    area.classList.add('active')
  })
  area.addEventListener('dragleave', () => area.classList.remove('active'))
  area.addEventListener('drop', (event) => {
    event.preventDefault()
    area.classList.remove('active')
    void handOver(Array.from(event.dataTransfer?.files ?? []))
  })
  // A file dropped beside the area would make the browser open it in place of this page.
  window.addEventListener('dragover', (event) => event.preventDefault())
  window.addEventListener('drop', (event) => event.preventDefault())
}

// The page is the only consumer; a test imports the exports above and never runs this.
if (globalThis.document) init()
