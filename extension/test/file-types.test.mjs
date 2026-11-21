// The person's own list of file types (RD-1440-08): how an entry is read, how a download's type
// is matched, and what the interceptor does with the answer in each of the three modes.

import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  MAX_TYPES,
  MAX_TYPE_LENGTH,
  browserOnlyEntries,
  captureFilter,
  matchesType,
  normalizeExtension,
  normalizeExtensions,
  normalizeMode,
  parseExtensions,
  passesTypeFilter
} from '../src/file-types.js'
import { typesNotices } from '../src/options.js'
import { setupInterceptor as setup } from './fakes.mjs'

const DLC = { id: 1, url: 'https://share.test/get/42', filename: '/home/me/Downloads/release.dlc' }
const ZIP = { id: 2, url: 'https://files.test/pack.zip', filename: 'pack.zip' }
const MKV = { id: 3, url: 'https://files.test/movie.mkv', filename: 'movie.mkv' }

test('.dlc, dlc, *.dlc and " .DLC " are one entry; empties and duplicates go', () => {
  for (const entry of ['.dlc', 'dlc', '*.dlc', ' .DLC ', '**DLC']) assert.equal(normalizeExtension(entry), '.dlc', entry)
  assert.equal(normalizeExtension('tar.gz'), '.tar.gz')
  assert.equal(normalizeExtension('.7z'), '.7z')
  for (const entry of ['', '   ', '*', '.', '..dlc', 'a/b', 'my file', '.dlc.', null, undefined]) {
    assert.equal(normalizeExtension(entry), null, String(entry))
  }
  assert.deepEqual(normalizeExtensions('dlc, *.DLC; .nzb\n.dlc  ,, zip'), ['.dlc', '.nzb', '.zip'])
  assert.deepEqual(normalizeExtensions(['.dlc', 'DLC', '', 7, null]), ['.dlc', '.7'])
  assert.deepEqual(normalizeExtensions(undefined), [])
})

test('an entry is capped in length and the list in size', () => {
  const longest = `.${'a'.repeat(MAX_TYPE_LENGTH - 1)}`
  assert.equal(normalizeExtension(longest), longest)
  // Cut short it would name another type, so a longer one is dropped, not truncated.
  assert.equal(normalizeExtension(`${longest}b`), null)
  const many = Array.from({ length: MAX_TYPES + 5 }, (_, index) => `.t${index}`)
  const parsed = parseExtensions(many.join(' '))
  assert.equal(parsed.extensions.length, MAX_TYPES)
  assert.deepEqual(parsed.extensions, many.slice(0, MAX_TYPES))
  assert.equal(parsed.overflow, 5)
  // A duplicate beyond the cap is still a duplicate, not an overflow.
  assert.equal(parseExtensions([...many.slice(0, MAX_TYPES), '.t0']).overflow, 0)
  assert.deepEqual(parseExtensions('dlc no/pe x..y dlc no/pe').invalid, ['no/pe', 'x..y'])
})

test('an unknown or missing mode is "all", and a stored configuration reads as a filter', () => {
  for (const mode of ['all', 'only', 'except']) assert.equal(normalizeMode(mode), mode)
  for (const mode of [undefined, null, '', 'ONLY', 'none']) assert.equal(normalizeMode(mode), 'all')
  assert.deepEqual(captureFilter({}), { mode: 'all', extensions: [] })
  assert.deepEqual(captureFilter({ interceptMode: 'only', interceptTypes: ['DLC', '*.ccf'] }), { mode: 'only', extensions: ['.dlc', '.ccf'] })
})

test('a type is read from the name, the address, the Content-Disposition and the content type', () => {
  const plain = { url: 'https://share.test/get/42', filename: '', mime: 'application/octet-stream' }
  assert.equal(matchesType(plain, null, ['.dlc']), false)
  assert.equal(matchesType({ ...plain, filename: 'C:\\Downloads\\Release.DLC' }, null, ['.dlc']), true)
  assert.equal(matchesType({ ...plain, url: 'https://share.test/a/release.dlc?x=1' }, null, ['.dlc']), true)
  assert.equal(matchesType({ ...plain, finalUrl: 'https://cdn.test/release.dlc' }, null, ['.dlc']), true)
  assert.equal(matchesType(plain, { contentDisposition: 'attachment; filename="release.dlc"' }, ['.dlc']), true)
  assert.equal(matchesType({ ...plain, mime: 'application/x-dlc; charset=binary' }, null, ['.dlc']), true)
  assert.equal(matchesType({ ...plain, mime: 'application/x-bittorrent' }, null, ['.torrent']), true)
  assert.equal(matchesType({ ...plain, mime: 'application/x-bittorrent' }, null, ['.dlc']), false)
  assert.equal(matchesType({ ...plain, filename: 'backup.tar.gz' }, null, ['.gz']), true)
  // The ending, not a word somewhere in it.
  assert.equal(matchesType({ ...plain, filename: 'dlc-howto.txt', url: 'https://x.test/dlc/page' }, null, ['.dlc']), false)
  assert.equal(matchesType(DLC, null, []), false)
})

test('"only .dlc": a .dlc goes, a .zip and a .mkv stay; "all except .zip" the reverse', () => {
  const only = { mode: 'only', extensions: ['.dlc'] }
  assert.equal(passesTypeFilter(DLC, null, only), true)
  assert.equal(passesTypeFilter(ZIP, null, only), false)
  assert.equal(passesTypeFilter(MKV, null, only), false)
  const except = { mode: 'except', extensions: ['.zip'] }
  assert.equal(passesTypeFilter(DLC, null, except), true)
  assert.equal(passesTypeFilter(ZIP, null, except), false)
  assert.equal(passesTypeFilter(MKV, null, except), true)
  // An empty "only" list takes nothing, an empty "except" list everything; "all" ignores the list.
  assert.equal(passesTypeFilter(MKV, null, { mode: 'only', extensions: [] }), false)
  assert.equal(passesTypeFilter(MKV, null, { mode: 'except', extensions: [] }), true)
  for (const item of [DLC, ZIP, MKV]) assert.equal(passesTypeFilter(item, null, { mode: 'all', extensions: ['.mkv'] }), true)
  assert.equal(passesTypeFilter(MKV, null, undefined), true)
})

/** The interceptor with a file handover that records what reached it. */
function intercept(config) {
  const calls = []
  const browserOnly = []
  const files = { onBrowserOnlyDownload: async (item) => { browserOnly.push(item.id); return true } }
  const { interceptor, bodies } = setup({ calls, config, files })
  return { interceptor, calls, bodies, browserOnly }
}

async function run(config) {
  const outcome = intercept(config)
  for (const item of [DLC, ZIP, MKV]) await outcome.interceptor.onDownloadCreated(item)
  return { ...outcome, handedOver: outcome.bodies.map((body) => body.links[0].url) }
}

test('mode "all" takes every download the way it did before the list existed', async () => {
  for (const config of [{}, { interceptMode: 'all', interceptTypes: ['.mkv'] }]) {
    const { handedOver, browserOnly } = await run(config)
    assert.deepEqual(handedOver, [MKV.url])
    // A .dlc and a .zip are browser-only files, and take the file handover's way as before.
    assert.deepEqual(browserOnly, [DLC.id, ZIP.id])
  }
})

test('"only .dlc" sends the .dlc its way and leaves the .zip and the .mkv to the browser untouched', async () => {
  const { handedOver, browserOnly, calls } = await run({ interceptMode: 'only', interceptTypes: ['.dlc'] })
  assert.deepEqual(handedOver, [])
  assert.deepEqual(browserOnly, [DLC.id])
  assert.deepEqual(calls, [], 'nothing paused, nothing said')
})

test('"all except .zip" hands the .mkv over and the .dlc its way, and leaves the .zip alone', async () => {
  const { handedOver, browserOnly } = await run({ interceptMode: 'except', interceptTypes: ['zip'] })
  assert.deepEqual(handedOver, [MKV.url])
  assert.deepEqual(browserOnly, [DLC.id])
})

test('a download the list leaves out is not even refused as a POST', async () => {
  const { interceptor, calls } = intercept({ interceptMode: 'only', interceptTypes: ['.dlc'] })
  interceptor.onBeforeRequest({ url: MKV.url, method: 'POST' })
  await interceptor.onDownloadCreated(MKV)
  assert.deepEqual(calls, [])
})

test('the options page names the selected types the browser downloads itself', () => {
  assert.deepEqual(browserOnlyEntries(['.dlc', '.mkv', '.nzb', '.tar.zip']), ['.dlc', '.nzb', '.tar.zip'])
  assert.deepEqual(typesNotices('only', '.dlc .mkv'), [{ key: 'optionsInterceptTypesBrowserOnly', substitutions: ['.dlc'] }])
  // Excluded, they stay in the browser anyway; and "all" says nothing about a list it ignores.
  assert.deepEqual(typesNotices('except', '.dlc'), [])
  assert.deepEqual(typesNotices('only', ''), [{ key: 'optionsInterceptTypesEmpty', substitutions: undefined }])
  assert.deepEqual(typesNotices('except', 'a/b .mkv'), [{ key: 'optionsInterceptTypesInvalid', substitutions: ['a/b'] }])
  const many = Array.from({ length: MAX_TYPES + 1 }, (_, index) => `.t${index}`).join(' ')
  assert.deepEqual(typesNotices('except', many), [{ key: 'optionsInterceptTypesTooMany', substitutions: [String(MAX_TYPES)] }])
})
