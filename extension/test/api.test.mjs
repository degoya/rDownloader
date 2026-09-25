import assert from 'node:assert/strict'
import { test } from 'node:test'

import { hostPattern, isLoopback, normalizeServer, ping, submitLinks } from '../src/api.js'

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
