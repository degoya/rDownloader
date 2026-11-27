// Click'n'Load through the browser extension, end to end in Chromium (RD-1260-10): a hoster page
// with a Click'n'Load form shows its button although no agent listens on 127.0.0.1:9666, and a
// click brings its links, package name and password into the LinkGrabber of a fresh service. Then
// a stand-in agent takes the port, and the extension stays out of its way. Since RD-1490-13 also a
// DLC from an allowed page, the whole way into the LinkGrabber (RD-1260-11): once built by the page
// as a `blob:` download, which goes over as its bytes, and once behind a link and a login, which
// goes over as its address with the session's cookies — both decrypted by a local stand-in for the
// decryption service.
//
//   RD_E2E_SERVER=…/rdownloader RD_E2E_EXTENSION=artifacts/browser-extensions/chrome \
//     node --test web/e2e/click-and-load.e2e.mjs
//
// The hoster is a local page under the name `hoster.test` (Chromium's host resolver maps it). A
// site is allowed in the popup with a click and a browser prompt no automation can answer, so the
// run loads a copy of the extension whose manifest grants `*://hoster.test/*` at install, and
// records the consent in storage as the popup would. Everything else is the shipped build.
// Firefox's half is firefox.e2e.mjs.
//
// The linked DLC's site is this machine under its private IPv4 address: the service fetches a
// handed-over address itself and refuses loopback, and it cannot resolve `hoster.test`.
import assert from 'node:assert/strict'
import { cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { after, before, test } from 'node:test'

import { chromium } from 'playwright'

import { CNL_PORT, encrypt, hosterPage, startAgent, startHoster } from './lib/click-and-load.mjs'
import { blobPage, dlcContainer, privateAddress, startDlcService, startDlcSite } from './lib/dlc.mjs'
import { BUDGET_MS, Session, assertPortFree, marker, startService, stop, within } from './lib/service.mjs'

const SERVER = process.env.RD_E2E_SERVER
const EXTENSION = process.env.RD_E2E_EXTENSION && resolve(process.env.RD_E2E_EXTENSION)
const WORK = process.env.RD_E2E_WORK || mkdtempSync(join(tmpdir(), 'rd-e2e-cnl-'))
const PORT = 8710

let service
let session
let context
let extensionId
let hoster
let agent
let dlcService
let dlcSite
const problems = []

/**
 * The shipped Chrome build, granted at install what allowing the test sites in the popup grants:
 * each host, and `cookies` (`sitePermissions` in `src/files.js`).
 */
function extensionCopy(hosts) {
  const copy = join(WORK, 'extension')
  cpSync(EXTENSION, copy, { recursive: true })
  const manifestPath = join(copy, 'manifest.json')
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'))
  manifest.host_permissions = [...manifest.host_permissions, ...hosts.map((host) => `*://${host}/*`)]
  manifest.permissions = [...manifest.permissions, 'cookies']
  manifest.optional_permissions = (manifest.optional_permissions ?? []).filter((permission) => permission !== 'cookies')
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
  const siteAddress = privateAddress()
  if (siteAddress) dlcSite = await startDlcSite(siteAddress)
  const extension = extensionCopy(['hoster.test', ...(dlcSite ? [dlcSite.host] : [])])
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
  await dlcSite?.close()
  await dlcService?.close()
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

/**
 * Both links of a container reach the LinkGrabber, the container itself does not go there as a
 * link, and the browser keeps no copy of it.
 */
async function assertImported(links, id, name) {
  const { elapsedMs } = await session.awaitCandidate(id)
  const { value: candidates } = await within(BUDGET_MS.linkArrival, `both links of ${name}`, async () => {
    const candidates = await session.expect(200, 'GET', '/api/v1/collector/candidates')
    return links.every((link) => candidates.some((candidate) => candidate.url === link)) && candidates
  })
  assert.ok(!candidates.some((candidate) => String(candidate.url).includes(name)), 'the container went over as a link')
  await within(BUDGET_MS.linkArrival, `the browser keeping no copy of ${name}`, async () => {
    const copies = await inExtension(
      async (file) => (await chrome.downloads.search({})).filter((item) => `${item.url} ${item.filename}`.includes(file)).length,
      name
    )
    return copies === 0
  })
  return elapsedMs
}

test('a DLC on an allowed page reaches the LinkGrabber through the extension (RD-1490-13)', { timeout: 180_000 }, async (t) => {
  assert.ok(dlcSite, 'this machine has no private IPv4 address for the site with the linked DLC')
  const bearer = await session.pairCapture('E2E DLC')
  // DLC import is off by default and needs its service; the stand-in answers on loopback.
  dlcService = await startDlcService()
  const settings = await session.expect(200, 'GET', '/api/v1/settings')
  await session.expect(200, 'PUT', '/api/v1/settings', {
    ...settings,
    dlc_service_enabled: true,
    dlc_service_endpoint: dlcService.endpoint
  })
  const sites = ['hoster.test', dlcSite.host]
  await inExtension((config) => chrome.storage.local.set(config), { server: service.base, token: bearer, fileSites: sites })
  // The page script goes into an allowed site's pages from their start, once it is registered.
  await within(BUDGET_MS.linkArrival, 'the page scripts for both allowed sites', async () => {
    const scripts = await inExtension(() => chrome.scripting.getRegisteredContentScripts())
    return scripts.length === 2 && scripts.every((script) => sites.every((host) => script.matches.includes(`*://${host}/*`)))
  }, 500)

  await t.test('a container the page builds goes over as its bytes', async () => {
    const id = marker('blob')
    const links = [`https://example.invalid/${id}.part1.rar`, `https://example.invalid/${id}.part2.rar`]
    const dlc = dlcContainer(links, `E2E ${id}`)
    dlcService.register(dlc)
    const name = `${marker('built')}.dlc`
    hoster.serve(blobPage(dlc.container, name))
    const page = await context.newPage()
    await page.goto('http://hoster.test/')
    await page.click('#dlc')
    const elapsedMs = await assertImported(links, id, name)
    console.log(`DLC built by an allowed page in the LinkGrabber after ${elapsedMs} ms`)
    await page.close()
  })

  await t.test('a linked container behind a login goes over as its address with the cookies', async () => {
    const id = marker('link')
    const links = [`https://example.invalid/${id}.part1.rar`, `https://example.invalid/${id}.part2.rar`]
    const dlc = dlcContainer(links, `E2E ${id}`)
    dlcService.register(dlc)
    const name = `${marker('linked')}.dlc`
    dlcSite.offer(name, dlc.container)
    dlcSite.serve(`<!doctype html>\n<meta charset="utf-8"><title>DLC test</title>\n<a id="dlc" href="/${name}">${name}</a>`)
    const page = await context.newPage()
    await page.goto(`${dlcSite.origin}/`)
    await page.click('#dlc')
    const elapsedMs = await assertImported(links, id, name)
    // Only the cookie opens the file: the service's fetch carried the browser's session.
    assert.equal(dlcSite.refused(), 0, 'a request for the container came without the session cookie')
    console.log(`DLC linked from an allowed page in the LinkGrabber after ${elapsedMs} ms`)
    await page.close()
  })

  assert.deepEqual(problems, [], 'the extension reported errors')
})
