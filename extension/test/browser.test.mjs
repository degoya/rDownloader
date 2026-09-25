// The three functions every setting and every visible string of the extension goes through.
// None of them had a test (RD-109-26).

import assert from 'node:assert/strict'
import { test } from 'node:test'

const stored = {}
const catalogue = { extName: 'rDownloader', sentLinks: '$1 link(s) sent to the LinkGrabber.' }

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
    interceptDownloads: true
  })
})

test('saving and reading a configuration round-trips, and interception defaults to on', async () => {
  await saveConfig({ server: 'http://nas.local:8710', token: 'abc' })
  assert.deepEqual(await loadConfig(), {
    server: 'http://nas.local:8710',
    token: 'abc',
    interceptDownloads: true
  })
  // Only an explicit false switches it off — an absent value is not a refusal.
  await saveConfig({ server: 'http://nas.local:8710', token: 'abc', interceptDownloads: false })
  assert.equal((await loadConfig()).interceptDownloads, false)
  await saveConfig({ server: 'http://nas.local:8710', token: 'abc', interceptDownloads: true })
  assert.equal((await loadConfig()).interceptDownloads, true)
})

test('a missing translation shows its key rather than an empty label', () => {
  assert.equal(message('extName'), 'rDownloader')
  assert.equal(message('sentLinks', ['3']), '3 link(s) sent to the LinkGrabber.')
  // `getMessage` answers '' for a key it does not know; an empty label reads as a broken UI,
  // the key at least says which string is missing.
  assert.equal(message('somethingNobodyTranslated'), 'somethingNobodyTranslated')
})
