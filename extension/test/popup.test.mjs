// The popup's decisions, without a DOM. `popup.js` had no exports at all, so the refused
// clipboard, the clipboard without links and the 401 suppression in the captcha list were
// untestable and untested (RD-109-26).

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { captchaListFailure, clipboardLinks, sendablePageUrl, versionLine } from '../src/popup.js'

test('a refused clipboard and an empty one are told apart', async () => {
  assert.deepEqual(
    await clipboardLinks(async () => { throw new Error('permission denied') }),
    { status: 'clipboardDenied' }
  )
  assert.deepEqual(await clipboardLinks(async () => 'just some prose'), { status: 'clipboardNoLinks' })
  assert.deepEqual(await clipboardLinks(async () => ''), { status: 'clipboardNoLinks' })
})

test('a clipboard with links hands them over as they are', async () => {
  const text = 'https://hoster.test/a\nhttps://hoster.test/b'
  assert.deepEqual(await clipboardLinks(async () => text), { text })
  // Any scheme spelling a person might paste, and text around the links, still counts.
  assert.deepEqual(
    await clipboardLinks(async () => 'see HTTPS://hoster.test/a for the file'),
    { text: 'see HTTPS://hoster.test/a for the file' }
  )
})

test('an unpaired extension gets no captcha error line', () => {
  // A 401 is the unpaired state, which the options page already explains; saying it again on
  // every popup open is noise.
  assert.equal(captchaListFailure({ ok: false, status: 401, message: 'nope' }), null)
  assert.equal(captchaListFailure({ ok: true, widgets: [] }), null)
  assert.deepEqual(captchaListFailure({ ok: false, status: 0, message: 'connection refused' }), {
    key: 'captchaListFailed',
    substitutions: ['connection refused']
  })
  assert.deepEqual(captchaListFailure({ ok: false, status: 500 }), {
    key: 'captchaListFailed',
    substitutions: ['']
  })
  // The listing comes from the background now (RD-109-23), which calls the same state
  // `unconfigured` rather than answering with the server's 401.
  assert.equal(captchaListFailure({ ok: false, code: 'unconfigured', widgets: [] }), null)
  // A background that could not be reached is not the unpaired state and is worth saying,
  // even though it carries no message of its own.
  assert.deepEqual(captchaListFailure({ ok: false, status: 0, code: 'background', message: '' }), {
    key: 'captchaListFailed',
    substitutions: ['background']
  })
})

test('only a tab with a web address can be sent', () => {
  assert.equal(sendablePageUrl({ url: 'https://hoster.test/files' }), 'https://hoster.test/files')
  assert.equal(sendablePageUrl({ url: 'http://hoster.test/files' }), 'http://hoster.test/files')
  // `activeTab` gives no url until the person clicks, and an internal page has none to send.
  assert.equal(sendablePageUrl({}), null)
  assert.equal(sendablePageUrl(undefined), null)
  assert.equal(sendablePageUrl({ url: 'chrome://extensions' }), null)
  assert.equal(sendablePageUrl({ url: 'about:blank' }), null)
})

/**
 * The version a person sees is the one the build carries.
 *
 * Read from the manifest rather than restated in the page: a second source would be a second
 * thing to keep in step, and RD-109-17 made the manifest the place that holds the release
 * version at all. A build without one shows nothing instead of a half-line (RD-109-46).
 */
test('the popup names the version the manifest carries', () => {
  assert.equal(versionLine(() => ({ version: '1.0.9' })), 'v1.0.9')
})

test('a manifest without a version leaves the line empty rather than half-written', () => {
  assert.equal(versionLine(() => ({})), '')
  assert.equal(versionLine(() => null), '')
  assert.equal(versionLine(() => { throw new Error('no runtime') }), '')
})
