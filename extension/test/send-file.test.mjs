// "Send file to rDownloader…" (RD-1260-11): local files chosen or dropped on the extension's own
// page go to `capture/file` one by one; an unknown type is refused before anything is sent, under
// a stable code and by name, and the whole selection gets one notice.

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { ACCEPTED_EXTENSIONS, UNKNOWN_EXTENSION, acceptedName, sendFiles, summary } from '../src/send-file.js'

const CONFIG = { server: 'http://127.0.0.1:8710', token: 'capture-token' }

const translate = (key, substitutions) => (substitutions ? `${key}:${substitutions.join(',')}` : key)

function file(name, content = 'x') {
  return new File([content], name)
}

test('the page takes what capture/file takes: NZBs, torrents, ZIPs and the link containers', () => {
  assert.deepEqual(ACCEPTED_EXTENSIONS, ['.nzb', '.torrent', '.zip', '.dlc', '.ccf', '.rsdf', '.rdlinks', '.crawljob'])
  for (const name of ['a.nzb', 'B.TORRENT', 'cart.zip', 'Season.DLC', 'x.ccf', 'x.rsdf', 'x.rdlinks', 'job.crawljob']) {
    assert.equal(acceptedName(name), true, name)
  }
  for (const name of ['readme.txt', 'movie.mkv', '.dlc', '', 'dlc', 'release.nzb.part']) {
    assert.equal(acceptedName(name), false, name)
  }
})

test('several files go over one by one, an unknown one never leaves the browser', async () => {
  const submitted = []
  const submit = async (config, body) => {
    submitted.push({ config, name: body.fileName, text: await body.bytes.text() })
    return { ok: true, status: 201, code: null, message: null, kind: 'container' }
  }
  const results = await sendFiles(CONFIG, [file('Season.dlc', 'QUJD'), file('notes.rar'), file('Release.nzb', '<nzb/>')], submit)
  assert.deepEqual(submitted, [
    { config: CONFIG, name: 'Season.dlc', text: 'QUJD' },
    { config: CONFIG, name: 'Release.nzb', text: '<nzb/>' }
  ])
  assert.deepEqual(results.map((result) => [result.name, result.ok, result.code]), [
    ['Season.dlc', true, null],
    ['notes.rar', false, UNKNOWN_EXTENSION],
    ['Release.nzb', true, null]
  ])
  // The capture agent's code for the same refusal (RD-1260-09).
  assert.equal(UNKNOWN_EXTENSION, 'capture.file_type_unsupported')
})

test('one notice names what went over, what was unknown and what the service refused', () => {
  const notice = summary([
    { name: 'a.dlc', ok: true, code: null },
    { name: 'b.nzb', ok: true, code: null },
    { name: 'c.exe', ok: false, code: UNKNOWN_EXTENSION },
    { name: 'd.rar', ok: false, code: UNKNOWN_EXTENSION },
    { name: 'e.dlc', ok: false, status: 400, code: 'dlc.service_disabled', message: 'DLC import is switched off' },
    { name: 'f.ccf', ok: false, status: 401, code: 'auth.unauthorized', message: 'HTTP 401' }
  ], translate)
  assert.deepEqual(notice.parts, [
    { key: 'sendFileSent', substitutions: ['2'] },
    { key: 'sendFileUnknown', substitutions: ['c.exe, d.rar'] },
    { key: 'sendFileRefused', substitutions: ['e.dlc (DLC import is switched off), f.ccf (errorUnauthorized)'] }
  ])
  assert.equal(notice.ok, false)
  assert.deepEqual(summary([{ name: 'a.nzb', ok: true, code: null }], translate), {
    parts: [{ key: 'sendFileSent', substitutions: ['1'] }],
    ok: true
  })
})
