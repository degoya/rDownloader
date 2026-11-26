// The three functions every setting and every visible string of the extension goes through.
// None of them had a test (RD-109-26).

import assert from 'node:assert/strict'
import { test } from 'node:test'

const stored = {}
const catalogue = { extName: 'rDownloader', sentLinks: '$1 links sent to the LinkGrabber.', sentLinksOne: '$1 link sent to the LinkGrabber.' }

// `browser` and `chrome` both present: Firefox's namespace is the one that must win, because it
// is the promise-based one this code is written against.
globalThis.chrome = { storage: { local: { get: async () => ({}), set: async () => {} } } }
globalThis.browser = {
  storage: {
    local: {
      get: async (defaults) => ({ ...defaults, ...stored }),
      set: async (values) => Object.assign(stored, values)
    }
  },
  i18n: {
    getMessage: (key, substitutions) => {
      const text = catalogue[key]
      if (text === undefined) return ''
      return substitutions ? text.replace('$1', substitutions[0]) : text
    }
  }
}

const { api, loadConfig, message, saveConfig } = await import('../src/browser.js')

test('the promise-based namespace wins where both exist', () => {
  assert.equal(api, globalThis.browser)
})

test('an unconfigured extension reads the defaults it was built with', async () => {
  assert.deepEqual(await loadConfig(), {
    server: 'http://127.0.0.1:8710',
    token: '',
    interceptDownloads: true,
    interceptMode: 'all',
    interceptTypes: [],
    clickAndLoad: true
  })
})

test('saving and reading a configuration round-trips, and interception defaults to on', async () => {
  await saveConfig({ server: 'http://nas.local:8710', token: 'abc' })
  assert.deepEqual(await loadConfig(), {
    server: 'http://nas.local:8710',
    token: 'abc',
    interceptDownloads: true,
    interceptMode: 'all',
    interceptTypes: [],
    clickAndLoad: true
  })
  // Only an explicit false switches it off — an absent value is not a refusal.
  await saveConfig({ server: 'http://nas.local:8710', token: 'abc', interceptDownloads: false })
  assert.equal((await loadConfig()).interceptDownloads, false)
  await saveConfig({ server: 'http://nas.local:8710', token: 'abc', interceptDownloads: true })
  assert.equal((await loadConfig()).interceptDownloads, true)
  // Click'n'Load without the agent (RD-1260-10) is on the same way, off only when switched off.
  await saveConfig({ server: 'http://nas.local:8710', token: 'abc', clickAndLoad: false })
  assert.equal((await loadConfig()).clickAndLoad, false)
  await saveConfig({ server: 'http://nas.local:8710', token: 'abc' })
  assert.equal((await loadConfig()).clickAndLoad, true)
})

test('the list of file types is stored normalised, and a broken mode reads as "all" (RD-1440-08)', async () => {
  await saveConfig({ server: 'http://nas.local:8710', token: 'abc', interceptMode: 'only', interceptTypes: ['*.DLC', 'dlc', '', 'a/b'] })
  const config = await loadConfig()
  assert.equal(config.interceptMode, 'only')
  assert.deepEqual(config.interceptTypes, ['.dlc'])
  assert.deepEqual(stored.interceptTypes, ['.dlc'], 'what reaches storage is the normalised list')
  stored.interceptMode = 'sometimes'
  stored.interceptTypes = 'not a list'
  assert.deepEqual(await loadConfig().then(({ interceptMode, interceptTypes }) => ({ interceptMode, interceptTypes })), { interceptMode: 'all', interceptTypes: [] })
  await saveConfig({ server: 'http://nas.local:8710', token: 'abc' })
})

test('a missing translation shows its key rather than an empty label', () => {
  assert.equal(message('extName'), 'rDownloader')
  assert.equal(message('sentLinks', ['3']), '3 links sent to the LinkGrabber.')
  // `getMessage` answers '' for a key it does not know; an empty label reads as a broken UI,
  // the key at least says which string is missing.
  assert.equal(message('somethingNobodyTranslated'), 'somethingNobodyTranslated')
})

test('a counted message follows the plural rule of its catalogue (RD-1480-03)', () => {
  // A catalogue that names no language is counted as English.
  assert.equal(message('sentLinks', ['1']), '1 link sent to the LinkGrabber.')
  assert.equal(message('sentLinks', ['0']), '0 links sent to the LinkGrabber.')
  catalogue.pageLanguage = 'fr'
  assert.equal(message('sentLinks', ['0']), '0 link sent to the LinkGrabber.', 'zero is singular in French')
  assert.equal(message('sentLinks', ['2']), '2 links sent to the LinkGrabber.')
  delete catalogue.pageLanguage
})
