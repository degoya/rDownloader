import { hostPattern, isLoopback, normalizeServer, ping } from './api.js'
import { api, loadConfig, message, saveConfig } from './browser.js'
import { loadSites, revokeSite } from './files.js'

/**
 * Makes sure the host permission this server needs is held.
 *
 * Returns `'granted'`, `'denied'`, `'deniedLoopback'` or `'invalid'`.
 *
 * There is no loopback shortcut any more. The manifest declares `http://127.0.0.1:8710/*` and
 * `http://localhost:8710/*` and nothing else, so a service on `http://127.0.0.1:9000` held no
 * host permission at all and worked only because rDownloader answers
 * `Access-Control-Allow-Origin: *` — an undocumented dependency on a server header
 * (RD-109-24). `permissions.contains` is what actually knows: for the two declared addresses it
 * answers yes and nothing is asked, for any other port it asks like it would for a remote host.
 */
export async function ensurePermission(server, permissions = api.permissions) {
  const pattern = hostPattern(server)
  if (!pattern) return 'invalid'
  try {
    if (await permissions.contains({ origins: [pattern] })) return 'granted'
    if (await permissions.request({ origins: [pattern] })) return 'granted'
  } catch {
    return 'invalid'
  }
  return isLoopback(server) ? 'deniedLoopback' : 'denied'
}

/** The message key and tone for the outcome of a connection test. */
export function connectionStatus(server, result) {
  if (!hostPattern(server)) return { key: 'serverInvalid', substitutions: undefined, ok: false }
  if (result.ok) return { key: 'connected', substitutions: [result.version ?? '?'], ok: true }
  if (result.status === 401) return { key: 'errorUnauthorized', substitutions: undefined, ok: false }
  return { key: 'sendFailed', substitutions: [result.message ?? ''], ok: false }
}

/** The message key and tone for the outcome of a save. */
export function saveStatus(outcome) {
  if (outcome === 'granted') return { key: 'saved', ok: true }
  if (outcome === 'invalid') return { key: 'serverInvalid', ok: false }
  if (outcome === 'deniedLoopback') return { key: 'permissionDeniedLoopback', ok: false }
  return { key: 'permissionDenied', ok: false }
}

function localize() {
  for (const element of document.querySelectorAll('[data-i18n]')) {
    element.textContent = message(element.dataset.i18n)
  }
  document.title = message('extName')
}

function setStatus(text, ok) {
  const status = document.getElementById('status')
  status.textContent = text
  status.className = ok ? 'ok' : 'error'
}

/**
 * The sites allowed to hand NZB, torrent and ZIP files over (RD-130-16), each with the button
 * that takes the consent back. A consent is given in the popup, on the site itself.
 */
async function renderFileSites() {
  const list = document.getElementById('file-sites')
  const sites = await loadSites(api)
  list.replaceChildren()
  for (const host of sites) {
    const item = document.createElement('li')
    const name = document.createElement('span')
    name.textContent = host
    const button = document.createElement('button')
    button.className = 'secondary'
    button.textContent = message('optionsFilesRevoke')
    button.addEventListener('click', async () => {
      await revokeSite(api, host)
      setStatus(message('filesRevoked', [host]), true)
      await renderFileSites()
    })
    item.append(name, button)
    list.append(item)
  }
  document.getElementById('file-sites-none').hidden = sites.length > 0
}

async function init() {
  localize()
  void renderFileSites()
  const config = await loadConfig()
  document.getElementById('server').value = config.server
  document.getElementById('token').value = config.token
  document.getElementById('intercept').checked = config.interceptDownloads !== false
  document.getElementById('save').addEventListener('click', async () => {
    const server = normalizeServer(document.getElementById('server').value)
    const token = document.getElementById('token').value.trim()
    const interceptDownloads = document.getElementById('intercept').checked
    const outcome = await ensurePermission(server)
    const status = saveStatus(outcome)
    if (outcome !== 'granted') {
      setStatus(message(status.key), status.ok)
      return
    }
    await saveConfig({ server, token, interceptDownloads })
    document.getElementById('server').value = server
    setStatus(message(status.key), status.ok)
  })
  document.getElementById('test').addEventListener('click', async () => {
    const server = normalizeServer(document.getElementById('server').value)
    const token = document.getElementById('token').value.trim()
    if (!hostPattern(server)) {
      setStatus(message('serverInvalid'), false)
      return
    }
    setStatus(message('testing'), true)
    const status = connectionStatus(server, await ping({ server, token }))
    setStatus(message(status.key, status.substitutions), status.ok)
  })
}

// The options page is the only consumer; a test imports the exports above and never runs this.
if (globalThis.document) void init()
