import assert from 'node:assert/strict'
import { test } from 'node:test'

import { PICK_WAITING, hostPattern, isLoopback, normalizeServer, ping, request, sendsTokenInClear, submitLinks } from '../src/api.js'

test('normalises server urls', () => {
  assert.equal(normalizeServer(''), 'http://127.0.0.1:8710')
  assert.equal(normalizeServer('nas.local:8710/'), 'http://nas.local:8710')
  assert.equal(normalizeServer('https://dl.example.com//'), 'https://dl.example.com')
  assert.equal(hostPattern('http://nas.local:8710'), 'http://nas.local:8710/*')
  assert.equal(isLoopback('http://localhost:8710'), true)
  assert.equal(isLoopback('http://nas.local:8710'), false)
})

test('isLoopback knows the IPv6 spellings a browser actually produces', () => {
  // `new URL('http://[::1]').hostname` keeps the brackets, always — so a comparison against
  // the bare `::1` could never be true and has gone (RD-109-25).
  assert.equal(isLoopback('http://[::1]:8710'), true)
  assert.equal(isLoopback('http://[::1]'), true)
  assert.equal(isLoopback('[::1]:8710'), true)
  // Without the brackets it is not an address at all; that is an invalid input, not a loopback.
  assert.equal(isLoopback('::1'), false)
  assert.equal(isLoopback('http://::1'), false)
  assert.equal(hostPattern('http://[::1]:8710'), 'http://[::1]:8710/*')
  assert.equal(hostPattern('::1'), null)
})

test('the token is flagged as travelling in clear only over plain http to another machine', () => {
  // The options page warns for these: the capture token would cross the LAN readable.
  assert.equal(sendsTokenInClear('http://nas.local:8710'), true)
  assert.equal(sendsTokenInClear('192.168.1.20:8710'), true, 'an address without a scheme is http')
  assert.equal(sendsTokenInClear('https://nas.local'), false)
  assert.equal(sendsTokenInClear('http://127.0.0.1:8710'), false)
  assert.equal(sendsTokenInClear('http://localhost:9000'), false)
  assert.equal(sendsTokenInClear('http://[::1]:8710'), false)
  assert.equal(sendsTokenInClear('http://::1'), false, 'not an address at all')
})

test('submits links with bearer token and reads the candidate count', async () => {
  const calls = []
  const fetchImpl = async (url, init) => {
    calls.push({ url, init })
    return { ok: true, status: 201, json: async () => ({ candidates: [{}, {}] }) }
  }
  const result = await submitLinks({ server: 'http://nas.local:8710', token: 'abc' }, { text: 'https://x.test/a', packageName: 'Pkg' }, fetchImpl)
  assert.deepEqual(result, { ok: true, status: 201, code: null, message: null, links: 2 })
  assert.equal(calls[0].url, 'http://nas.local:8710/api/v1/capture/batches')
  assert.equal(calls[0].init.headers.authorization, 'Bearer abc')
  const body = JSON.parse(calls[0].init.body)
  assert.equal(body.source, 'browser_extension')
  assert.equal(body.package_name, 'Pkg')
})

test('a page waiting for a choice is a success, with the number of releases (RD-1190-17)', async () => {
  const waiting = async () => ({
    ok: false,
    status: 400,
    json: async () => ({ error: 'listed', code: 'site_rules.pick_waiting', params: { entries: '30', list: '19a-0', rule: 'serienjunkies.org' } })
  })
  const result = await submitLinks({ server: '', token: 'abc' }, { text: 'https://serienjunkies.org/serie/show/' }, waiting)
  assert.deepEqual(result, { ok: true, status: 400, code: PICK_WAITING, message: null, links: 0, entries: 30 })
})

test('maps failures to codes', async () => {
  const unauthorized = async () => ({ ok: false, status: 401, json: async () => ({ error: 'A valid capture token is required', code: 'capture.token_required' }) })
  const result = await submitLinks({ server: '', token: '' }, { text: 'x' }, unauthorized)
  assert.equal(result.ok, false)
  assert.equal(result.code, 'capture.token_required')
  const offline = async () => { throw new Error('connection refused') }
  const network = await ping({ server: '', token: '' }, offline)
  assert.equal(network.ok, false)
  assert.equal(network.status, 0)
})

test('one request for the capture surface: the caller\'s headers merge over the defaults (EXT-14)', async () => {
  // `handover-api.js` kept its own copy of this and dropped whatever headers its caller passed.
  const seen = []
  const fetchImpl = async (url, init) => {
    seen.push({ url, init })
    return { ok: true, status: 200, json: async () => ({ code: 'capture.ok' }) }
  }
  const config = { server: 'nas.local:8710', token: 'abc' }

  const json = await request(config, '/api/v1/capture/x', { method: 'POST', body: '{}', headers: { 'x-trace': '1' } }, fetchImpl)
  assert.deepEqual(json, { ok: true, status: 200, code: 'capture.ok', message: null, payload: { code: 'capture.ok' } })
  assert.equal(seen[0].url, 'http://nas.local:8710/api/v1/capture/x')
  assert.deepEqual(seen[0].init.headers, { authorization: 'Bearer abc', 'content-type': 'application/json', 'x-trace': '1' })

  // A form sets its own type, boundary included; a type here would break it.
  await request(config, '/api/v1/capture/file', { method: 'POST', body: new FormData() }, fetchImpl)
  assert.deepEqual(seen[1].init.headers, { authorization: 'Bearer abc' })
})

test('the request reports a refusal and a lost connection in one shape', async () => {
  const refused = async () => ({ ok: false, status: 401, json: async () => { throw new Error('no body') } })
  assert.deepEqual(await request({ server: '', token: '' }, '/p', {}, refused),
    { ok: false, status: 401, code: 'auth.unauthorized', message: 'HTTP 401', payload: null })
  const offline = async () => { throw new Error('connection refused') }
  assert.deepEqual(await request({ server: '', token: '' }, '/p', {}, offline),
    { ok: false, status: 0, code: 'network', message: 'connection refused', payload: null })
})
