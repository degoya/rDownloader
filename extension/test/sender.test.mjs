// Who may talk to the background (sender.js): the one check every message that acts on the
// service or on a grant goes through, in background.js, captcha.js and handover.js alike.

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { fromOwnPage } from '../src/sender.js'

const OWN = 'chrome-extension://abcdefghijklmnop/'

/** The parts of `runtime` the check reads. */
function runtime(base = OWN) {
  return { id: 'abcdefghijklmnop', getURL: path => `${base}${path}` }
}

test('the popup as a popup carries no tab and is our own page', () => {
  assert.equal(fromOwnPage(runtime(), { id: 'abcdefghijklmnop' }), true)
})

test('the popup opened as a tab is our own page by its address', () => {
  const sender = { id: 'abcdefghijklmnop', tab: { id: 7 }, url: `${OWN}popup.html` }
  assert.equal(fromOwnPage(runtime(), sender), true)
})

test('a content script in a website tab is not our own page', () => {
  const sender = { id: 'abcdefghijklmnop', tab: { id: 7 }, url: 'https://ddownload.com/login.html' }
  assert.equal(fromOwnPage(runtime(), sender), false)
})

test('a page whose address only resembles ours is refused', () => {
  for (const url of [
    'https://evil.example/chrome-extension://abcdefghijklmnop/popup.html',
    // Another extension whose id starts with ours: the base ends in a slash for this reason.
    'chrome-extension://abcdefghijklmnopq/popup.html',
    undefined,
    42
  ]) {
    assert.equal(fromOwnPage(runtime(), { id: 'abcdefghijklmnop', tab: { id: 7 }, url }), false, String(url))
  }
})

test('another extension, a missing sender and an unknown base are refused', () => {
  assert.equal(fromOwnPage(runtime(), { id: 'someone-else' }), false)
  assert.equal(fromOwnPage(runtime(), { tab: { id: 7 }, url: `${OWN}popup.html` }), false)
  assert.equal(fromOwnPage(runtime(), undefined), false)
  assert.equal(fromOwnPage(runtime(), null), false)
  // A runtime that cannot name its own base admits no tab at all, rather than every address.
  const sender = { id: 'abcdefghijklmnop', tab: { id: 7 }, url: 'https://ddownload.com/' }
  assert.equal(fromOwnPage(runtime(''), sender), false)
  assert.equal(fromOwnPage(runtime(''), { ...sender, url: '' }), false)
})
