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

/**
 * The messages that count something, with the key of their singular. `chrome.i18n` has no plural;
 * the first substitution is the number, and the rule is that of the catalogue's own language — in
 * French zero is singular (RD-1480-03).
 */
const SINGULAR = { sentLinks: 'sentLinksOne', pickWaiting: 'pickWaitingOne', cnlSent: 'cnlSentOne' }

/** The language the texts are in: the catalogue's own, English where the browser's is not one. */
function language() {
  return api.i18n.getMessage('pageLanguage') || 'en'
}

export function message(key, substitutions) {
  const one = SINGULAR[key]
  const form = one && new Intl.PluralRules(language()).select(Number(substitutions?.[0])) === 'one' ? one : key
  return api.i18n.getMessage(form, substitutions) || key
}

/**
 * Words one of the extension's pages: its texts, its title, and the language a screen reader
 * speaks them in. That is the catalogue's language and not `i18n.getUILanguage()`: an Italian
 * browser shows the English texts (RD-1480-03).
 */
export function localizePage(title) {
  document.documentElement.lang = language()
  for (const element of document.querySelectorAll('[data-i18n]')) {
    element.textContent = message(element.dataset.i18n)
  }
  document.title = title
}
