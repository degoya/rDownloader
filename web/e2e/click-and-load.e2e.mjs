// Click'n'Load through the browser extension, end to end in Chromium (RD-1260-10): a hoster page
// with a Click'n'Load form shows its button although no agent listens on 127.0.0.1:9666, and a
// click brings its links, package name and password into the LinkGrabber of a fresh service. Then
// a stand-in agent takes the port, and the extension stays out of its way.
//
//   RD_E2E_SERVER=…/rdownloader RD_E2E_EXTENSION=artifacts/browser-extensions/chrome \
//     node --test web/e2e/click-and-load.e2e.mjs
//
// The hoster is a local page under the name `hoster.test` (Chromium's host resolver maps it). A
// site is allowed in the popup with a click and a browser prompt no automation can answer, so the
// run loads a copy of the extension whose manifest grants `*://hoster.test/*` at install, and
// records the consent in storage as the popup would. Everything else is the shipped build.
// Firefox cannot load an unpacked extension under Playwright; its half is the live checklist in
// docs/development.md.
import assert from 'node:assert/strict'
import { cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { createCipheriv } from 'node:crypto'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { after, before, test } from 'node:test'

import { chromium } from 'playwright'

import { BUDGET_MS, Session, assertPortFree, marker, startService, stop, within } from './lib/service.mjs'

const SERVER = process.env.RD_E2E_SERVER
const EXTENSION = process.env.RD_E2E_EXTENSION && resolve(process.env.RD_E2E_EXTENSION)
const WORK = process.env.RD_E2E_WORK || mkdtempSync(join(tmpdir(), 'rd-e2e-cnl-'))
const PORT = 8710
const CNL_PORT = 9666
const KEY = 'abababababababababababababababab'

let service
let session
let context
let extensionId
let hoster
let agent
const problems = []

/** The links a page encrypts for Click'n'Load: AES-128-CBC with the key as its own IV. */
function encrypt(text) {
  const key = Buffer.from(KEY, 'hex')
  const cipher = createCipheriv('aes-128-cbc', key, key)
  return Buffer.concat([cipher.update(text, 'utf8'), cipher.final()]).toString('base64')
}

/** A hoster page as they are written: the check script, then a form the button submits. */
function hosterPage({ crypted, packageName, password }) {
  return `<!doctype html>
<meta charset="utf-8"><title>Click'n'Load test</title>
<script>var jdownloader = false</script>
<script src="http://127.0.0.1:${CNL_PORT}/jdcheck.js"></script>
<form id="cnl" action="http://127.0.0.1:${CNL_PORT}/flash/addcrypted2" method="post" target="sink">
  <input type="hidden" name="crypted" value="${crypted}">
  <input type="hidden" name="jk" value="function f(){ return '${KEY}'; }">
  <input type="hidden" name="passwords" value="${password}">
  <input type="hidden" name="package" value="${packageName}">
  <input type="hidden" name="source" value="http://hoster.test/">
  <button id="cnl-button" hidden>Click'n'Load</button>
</form>
<iframe name="sink"></iframe>
<script>if (window.jdownloader === true) document.getElementById('cnl-button').hidden = false</script>`
}

/** Serves whatever page the current step put there. */
async function startHoster() {
  let page = ''
  const server = createServer((request, response) => {
    response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
    response.end(page)
  })
  await new Promise((done) => server.listen(0, '127.0.0.1', done))
  return {
    port: server.address().port,
    serve: (html) => {
      page = html
    },
    close: () => new Promise((done) => server.close(done))
  }
}

/** A stand-in for the desktop agent on its port: it answers the check and keeps what it is sent. */
async function startAgent() {
  const received = []
  const server = createServer((request, response) => {
    let body = ''
    request.on('data', (chunk) => {
      body += chunk
    })
    request.on('end', () => {
      if (request.url.startsWith('/flash/add')) received.push(body)
      response.writeHead(200, { 'access-control-allow-origin': '*' })
      response.end(request.url === '/jdcheck.js' ? "jdownloader=true; var version='agent';" : 'success')
    })
  })
  await new Promise((done) => server.listen(CNL_PORT, '127.0.0.1', done))
  return { received, close: () => new Promise((done) => server.close(done)) }
}

/** The shipped Chrome build, granted the test hoster at install. */
function extensionCopy() {
  const copy = join(WORK, 'extension')
  cpSync(EXTENSION, copy, { recursive: true })
  const manifestPath = join(copy, 'manifest.json')
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'))
  manifest.host_permissions = [...manifest.host_permissions, '*://hoster.test/*']
  writeFileSync(manifestPath, JSON.stringify(manifest, null, 2))
  return copy
}

/** Runs `work` in an extension page, where `chrome.*` is the extension's own. */
async function inExtension(work, argument) {
  const page = await context.newPage()
  try {
    await page.goto(`chrome-extension://${extensionId}/src/options.html`)
    return await page.evaluate(work, argument)
  } finally {
    await page.close()
  }
}

async function sessionRuleCount() {
  return inExtension(async () => (await chrome.declarativeNetRequest.getSessionRules()).length)
}

before(async () => {
  assert.ok(SERVER, 'RD_E2E_SERVER names the rdownloader binary')
  assert.ok(EXTENSION, 'RD_E2E_EXTENSION names the unpacked Chrome extension')
  await assertPortFree(PORT)
  await assertPortFree(CNL_PORT)
  service = await startService({ binary: SERVER, port: PORT, workDir: join(WORK, 'service') })
  session = new Session(service.base)
  await session.signIn(`e2e-${marker('pw')}`)
  hoster = await startHoster()
  const extension = extensionCopy()
  context = await chromium.launchPersistentContext(join(WORK, 'profile'), {
    channel: 'chromium',
    headless: process.env.RD_E2E_HEADED !== '1',
    args: [
      `--disable-extensions-except=${extension}`,
      `--load-extension=${extension}`,
      `--host-resolver-rules=MAP hoster.test 127.0.0.1:${hoster.port}`
    ]
  })
  context.on('weberror', (error) => problems.push(`weberror: ${error.error().message}`))
  const worker = context.serviceWorkers()[0] ?? (await context.waitForEvent('serviceworker'))
  extensionId = new URL(worker.url()).host
  worker.on('console', (entry) => {
    if (entry.type() === 'error') problems.push(`service worker: ${entry.text()}`)
  })
})

after(async () => {
  await context?.close()
  await agent?.close()
  await hoster?.close()
  await stop(service?.child)
  console.log(`logs in ${WORK}`)
})

test("a Click'n'Load page works without the agent, and the agent keeps the port when it runs", { timeout: 180_000 }, async (t) => {
  const bearer = await session.pairCapture("E2E Click'n'Load")
  // What the options page saves, and the consent the popup records for an allowed site.
  await inExtension(
    (config) => chrome.storage.local.set(config),
    { server: service.base, token: bearer, clickAndLoad: true, fileSites: ['hoster.test'] }
  )
  await within(BUDGET_MS.linkArrival, 'the session rules for the allowed site', async () => (await sessionRuleCount()) === 3)

  await t.test('without the agent the page shows its button and the click reaches the LinkGrabber', async () => {
    const id = marker('cnl')
    const links = [`http://files.test/${id}.part1.rar`, `http://files.test/${id}.part2.rar`]
    const packageName = `Release ${id}`
    hoster.serve(hosterPage({ crypted: encrypt(links.join('\r\n')), packageName, password: 'secret-pw' }))
    const page = await context.newPage()
    await page.goto('http://hoster.test/')
    await page.waitForSelector('#cnl-button', { state: 'visible', timeout: BUDGET_MS.linkArrival })
    await page.click('#cnl-button')
    const { value, elapsedMs } = await session.awaitCandidate(id)
    assert.ok(links.includes(value.url), value.url)
    const packages = await session.expect(200, 'GET', '/api/v1/collector/packages')
    const handed = packages.find((entry) => entry.name === packageName)
    assert.ok(handed, `no package named ${packageName}`)
    assert.equal(handed.password, 'secret-pw')
    console.log(`Click'n'Load in the LinkGrabber after ${elapsedMs} ms`)
    await page.close()
  })

  await t.test('with an agent on the port the extension answers nothing and hands nothing over', async () => {
    agent = await startAgent()
    // A change of the switch makes the extension ask the port again, as its alarm would.
    await inExtension(async () => {
      await chrome.storage.local.set({ clickAndLoad: false })
      await chrome.storage.local.set({ clickAndLoad: true })
    })
    await within(BUDGET_MS.linkArrival, 'the session rules to be gone', async () => (await sessionRuleCount()) === 0)
    const id = marker('agent')
    hoster.serve(hosterPage({ crypted: encrypt(`http://files.test/${id}.rar`), packageName: `Agent ${id}`, password: 'pw' }))
    const page = await context.newPage()
    await page.goto('http://hoster.test/')
    await page.waitForSelector('#cnl-button', { state: 'visible', timeout: BUDGET_MS.linkArrival })
    await page.click('#cnl-button')
    await within(BUDGET_MS.linkArrival, 'the agent receiving the hand-over', async () => agent.received.length === 1)
    // Give a wrong hand-over the time it would need, then make sure there was none.
    await new Promise((done) => setTimeout(done, 2_000))
    const candidates = await session.expect(200, 'GET', '/api/v1/collector/candidates')
    assert.ok(!candidates.some((candidate) => String(candidate.url).includes(id)), 'the extension handed the agent\'s links over too')
    await page.close()
  })

  assert.deepEqual(problems, [], 'the extension reported errors')
})
