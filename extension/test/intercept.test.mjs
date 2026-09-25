import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  buildIntakePayload,
  buildLegacyPayload,
  correlateRequest,
  BROWSER_ONLY_EXTENSIONS,
  BROWSER_ONLY_TYPES,
  filterHeaders,
  shouldIntercept,
  staysInBrowser
} from '../src/intercept.js'

test('filterHeaders keeps only the allowlist and drops credentials', () => {
  const headers = filterHeaders([
    { name: 'Accept', value: 'application/pdf' },
    { name: 'Accept-Language', value: 'de-DE,de;q=0.9' },
    { name: 'Cookie', value: 'sid=1' },
    { name: 'Authorization', value: 'Bearer x' },
    { name: 'Proxy-Authorization', value: 'Basic y' },
    { name: 'X-Api-Key', value: 'z' },
    { name: 'Referer', value: 'https://example.com' },
    { name: 'User-Agent', value: 'Mozilla/5.0' }
  ])
  assert.deepStrictEqual(headers, [
    { name: 'accept', value: 'application/pdf' },
    { name: 'accept-language', value: 'de-DE,de;q=0.9' }
  ])
})

test('filterHeaders drops oversize values and caps the list', () => {
  assert.deepStrictEqual(filterHeaders([{ name: 'accept', value: 'a'.repeat(4097) }]), [])
  assert.deepStrictEqual(filterHeaders(undefined), [])
  const many = Array.from({ length: 40 }, () => ({ name: 'accept', value: '*/*' }))
  assert.equal(filterHeaders(many).length, 32)
})

test('shouldIntercept guards disabled, own downloads, the server origin and non-http schemes', () => {
  const options = { enabled: true, serverOrigin: 'http://127.0.0.1:8710', ownExtensionId: 'self' }
  const item = { url: 'https://files.example.com/report.pdf' }
  assert.equal(shouldIntercept(item, options), true)
  assert.equal(shouldIntercept(item, { ...options, enabled: false }), false)
  assert.equal(shouldIntercept({ ...item, byExtensionId: 'self' }, options), false)
  assert.equal(shouldIntercept({ ...item, byExtensionId: 'other' }, options), true)
  assert.equal(shouldIntercept({ url: 'http://127.0.0.1:8710/api/v1/files/1' }, options), false)
  assert.equal(shouldIntercept({ ...item, finalUrl: 'http://127.0.0.1:8710/x' }, options), false)
  assert.equal(shouldIntercept({ ...item, referrer: 'http://127.0.0.1:8710/downloads' }, options), false)
  assert.equal(shouldIntercept({ url: 'data:text/plain,hi' }, options), false)
  assert.equal(shouldIntercept({ url: 'blob:https://example.com/abc' }, options), false)
  assert.equal(shouldIntercept({ url: 'file:///tmp/a.txt' }, options), false)
  assert.equal(shouldIntercept({}, options), false)
})

test('staysInBrowser keeps every NZB, torrent and ZIP type in the browser, parameters and case aside', () => {
  const item = { url: 'https://indexer.test/getnzb/abc', filename: '' }
  assert.deepStrictEqual(BROWSER_ONLY_TYPES, [
    'application/x-nzb',
    'application/x-bittorrent',
    'application/zip',
    'application/x-zip-compressed'
  ])
  for (const mime of BROWSER_ONLY_TYPES) {
    assert.equal(staysInBrowser({ ...item, mime }), 'file', mime)
    assert.equal(staysInBrowser({ ...item, mime: `${mime.toUpperCase()}; charset=binary` }), 'file', mime)
  }
})

test('staysInBrowser keeps every NZB, torrent and ZIP extension, wherever the name comes from', () => {
  assert.deepStrictEqual(BROWSER_ONLY_EXTENSIONS, ['.nzb', '.torrent', '.zip'])
  const plain = { url: 'https://indexer.test/get/abc', filename: '' }
  for (const extension of BROWSER_ONLY_EXTENSIONS) {
    const name = `Some.Release${extension.toUpperCase()}`
    assert.equal(staysInBrowser({ ...plain, filename: `C:\\Users\\me\\Downloads\\${name}` }), 'file', `filename ${extension}`)
    assert.equal(staysInBrowser({ ...plain, url: `https://indexer.test/files/${name}?r=1#x` }), 'file', `url ${extension}`)
    assert.equal(staysInBrowser({ ...plain, finalUrl: `https://cdn.test/${encodeURIComponent(name)}` }), 'file', `finalUrl ${extension}`)
    for (const disposition of [
      `attachment; filename="${name}"`,
      `attachment; filename=${name}; size=10`,
      `attachment; filename*=UTF-8''${encodeURIComponent(name)}`
    ]) {
      assert.equal(staysInBrowser(plain, { contentDisposition: disposition }), 'file', disposition)
    }
  }
  // An extension is a suffix, not a substring.
  assert.equal(staysInBrowser({ ...plain, filename: 'nzb-guide.pdf', url: 'https://x.test/zip/torrent.html' }), null)
})

test('staysInBrowser keeps an observed POST in the browser', () => {
  const item = { url: 'https://files.test/report.pdf', filename: 'report.pdf' }
  assert.equal(staysInBrowser(item, { method: 'POST' }), 'post')
  assert.equal(staysInBrowser(item, { method: 'post', headers: [] }), 'post')
  // The file rule wins: an NZB behind a form is kept either way, and silently.
  assert.equal(staysInBrowser({ ...item, mime: 'application/x-nzb' }, { method: 'POST' }), 'file')
})

test('staysInBrowser still lets an ordinary file go, observed or not', () => {
  // Without host access nothing is observed and a POST cannot be told from a GET, so an
  // unobserved download goes over as before (RD-120-63, narrowed by the coordinator).
  const item = { url: 'https://files.test/report.pdf', filename: '/home/me/Downloads/report.pdf', mime: 'application/pdf' }
  assert.equal(staysInBrowser(item), null)
  assert.equal(staysInBrowser(item, null), null)
  assert.equal(staysInBrowser(item, { method: 'GET', contentDisposition: 'attachment; filename="report.pdf"' }), null)
  assert.equal(staysInBrowser({}), null)
  assert.equal(staysInBrowser({ url: 'not a url', filename: 'video.mkv', mime: 'application/octet-stream' }), null)
})

test('correlateRequest honours the window, prefers the newest and matches finalUrl', () => {
  const buffer = [
    { url: 'https://cdn.test/a', headers: [{ name: 'accept', value: 'old' }], timestamp: 1000 },
    { url: 'https://cdn.test/a', headers: [{ name: 'accept', value: 'new' }], timestamp: 4000 },
    { url: 'https://other.test/b', headers: [], timestamp: 4500 }
  ]
  const item = { url: 'https://files.test/a', finalUrl: 'https://cdn.test/a' }
  assert.equal(correlateRequest(buffer, item, 5000).headers[0].value, 'new')
  assert.equal(correlateRequest(buffer, item, 5000, 2000).headers[0].value, 'new')
  assert.equal(correlateRequest(buffer, item, 20000), null)
  assert.equal(correlateRequest(buffer, { url: 'https://nope.test/c' }, 5000), null)
  assert.equal(correlateRequest([], item, 5000), null)
})

test('buildIntakePayload produces the capture contract v1 body', () => {
  const payload = buildIntakePayload({
    item: {
      url: 'https://files.example.com/report.pdf',
      finalUrl: 'https://cdn.example.com/abc/report.pdf',
      referrer: 'https://example.com/downloads',
      filename: '/home/user/Downloads/report.pdf'
    },
    captured: {
      contentDisposition: 'attachment; filename="report.pdf"',
      headers: [
        { name: 'Accept', value: '*/*' },
        { name: 'Cookie', value: 'sid=1' }
      ]
    },
    userAgent: 'Mozilla/5.0 (X11)'
  })
  assert.deepStrictEqual(payload, {
    source: 'browser_download',
    source_label: 'Chrome',
    package_name: 'report.pdf',
    links: [
      {
        url: 'https://files.example.com/report.pdf',
        file_name: 'report.pdf',
        request: {
          method: 'GET',
          effective_url: 'https://cdn.example.com/abc/report.pdf',
          referrer: 'https://example.com/downloads',
          user_agent: 'Mozilla/5.0 (X11)',
          content_disposition: 'attachment; filename="report.pdf"',
          headers: [{ name: 'accept', value: '*/*' }]
        }
      }
    ]
  })
})

test('buildIntakePayload omits unknown fields and effective_url when the url did not change', () => {
  const payload = buildIntakePayload({
    item: { url: 'https://files.example.com/a.bin', finalUrl: 'https://files.example.com/a.bin', filename: '' },
    userAgent: 'Mozilla/5.0 (Firefox) Gecko Firefox/128.0'
  })
  assert.deepStrictEqual(payload, {
    source: 'browser_download',
    source_label: 'Firefox',
    links: [
      {
        url: 'https://files.example.com/a.bin',
        request: { method: 'GET', user_agent: 'Mozilla/5.0 (Firefox) Gecko Firefox/128.0' }
      }
    ]
  })
})

test('buildLegacyPayload sends text only', () => {
  assert.deepStrictEqual(buildLegacyPayload({ url: 'https://x.test/a.zip', filename: 'C:\\Users\\d\\a.zip' }), {
    text: 'https://x.test/a.zip',
    source: 'browser_extension',
    source_label: 'Browser',
    package_name: 'a.zip'
  })
  assert.deepStrictEqual(buildLegacyPayload({ url: 'https://x.test/a.zip' }), {
    text: 'https://x.test/a.zip',
    source: 'browser_extension',
    source_label: 'Browser'
  })
})
