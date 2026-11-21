// Cross-browser namespace with promise-based APIs (Firefox `browser`, Chrome `chrome`).
import { DEFAULT_CAPTURE_MODE, normalizeExtensions, normalizeMode } from './file-types.js'

export const api = globalThis.browser ?? globalThis.chrome

export async function loadConfig() {
  const stored = await api.storage.local.get({
    server: 'http://127.0.0.1:8710',
    token: '',
    interceptDownloads: true,
    interceptMode: DEFAULT_CAPTURE_MODE,
    interceptTypes: [],
    clickAndLoad: true
  })
  return {
    server: stored.server,
    token: stored.token,
    interceptDownloads: stored.interceptDownloads !== false,
    // Which downloads by their file type (RD-1440-08): every one unless a list says otherwise.
    interceptMode: normalizeMode(stored.interceptMode),
    interceptTypes: Array.isArray(stored.interceptTypes) ? normalizeExtensions(stored.interceptTypes) : [],
    // Click'n'Load without the desktop agent (RD-1260-10): on unless switched off.
    clickAndLoad: stored.clickAndLoad !== false
  }
}

export async function saveConfig(config) {
  await api.storage.local.set({
    server: config.server,
    token: config.token,
    interceptDownloads: config.interceptDownloads !== false,
    interceptMode: normalizeMode(config.interceptMode),
    interceptTypes: normalizeExtensions(config.interceptTypes ?? []),
    clickAndLoad: config.clickAndLoad !== false
  })
}

/**
 * Opens "Send file to rDownloader…" in a tab (RD-1260-11): a page, because Firefox closes the
 * popup the moment a file picker opens.
 */
export function openSendFilePage() {
  return api.tabs.create({ url: api.runtime.getURL('src/send-file.html') })
}

export function message(key, substitutions) {
  return api.i18n.getMessage(key, substitutions) || key
}
