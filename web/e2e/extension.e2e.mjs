// Browser extension end to end in Chromium (RD-180-12): the built Chrome extension loaded into a
// fresh profile, paired with a fresh service through its options page, and a page sent from the
// popup, asserted in the LinkGrabber.
//
//   RD_E2E_SERVER=…/rdownloader RD_E2E_EXTENSION=artifacts/browser-extensions/chrome \
//     node --test web/e2e/extension.e2e.mjs
//
// The service listens on 127.0.0.1:8710 because that is the address the manifest grants at
// install time; any other port makes Save ask for a host permission, and that prompt is browser
// chrome no automation can answer. Firefox cannot load an unpacked extension under Playwright;
// its half is `web-ext lint` in CI and the live checklist in docs/development.md.
import assert from 'node:assert/strict'
import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { after, before, test } from 'node:test'

import { chromium } from 'playwright'

import { BUDGET_MS, Session, assertPortFree, marker, startService, stop } from './lib/service.mjs'

const SERVER = process.env.RD_E2E_SERVER
const EXTENSION = process.env.RD_E2E_EXTENSION && resolve(process.env.RD_E2E_EXTENSION)
const WORK = process.env.RD_E2E_WORK || mkdtempSync(join(tmpdir(), 'rd-e2e-extension-'))
const PORT = 8710

let service
let session
let context
let extensionId
const problems = []

/** The extension's own text for `key`, so the assertions hold in whatever locale runs them. */
function localized(page, key, substitutions) {
  return page.evaluate(([name, values]) => chrome.i18n.getMessage(name, values), [key, substitutions])
}

before(async () => {
  assert.ok(SERVER, 'RD_E2E_SERVER names the rdownloader binary')
  assert.ok(EXTENSION, 'RD_E2E_EXTENSION names the unpacked Chrome extension')
  await assertPortFree(PORT)
  service = await startService({ binary: SERVER, port: PORT, workDir: join(WORK, 'service') })
  console.log(`service ${service.version} on ${service.base} after ${service.startMs} ms`)
  session = new Session(service.base)
  await session.signIn(`e2e-${marker('pw')}`)

  // The `chromium` channel is the full browser in its new headless mode; the headless shell
  // Playwright uses otherwise does not load extensions.
  context = await chromium.launchPersistentContext(join(WORK, 'profile'), {
    channel: 'chromium',
    headless: process.env.RD_E2E_HEADED !== '1',
    args: [`--disable-extensions-except=${EXTENSION}`, `--load-extension=${EXTENSION}`]
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
  await stop(service?.child)
  console.log(`logs in ${WORK}`)
})

test('the extension pairs through its options page and sends a page from the popup', { timeout: 120_000 }, async (t) => {
  const bearer = await session.pairCapture('E2E browser extension')

  await t.test('Test connection reaches the service and Save keeps the pairing', async () => {
    const options = await context.newPage()
    await options.goto(`chrome-extension://${extensionId}/src/options.html`)
    await options.fill('#server', service.base)
    await options.fill('#token', bearer)
    await options.click('#test')
    const connected = await localized(options, 'connected', [service.version])
    await assert.doesNotReject(
      options.waitForFunction((text) => document.getElementById('status')?.textContent === text, connected, {
        timeout: BUDGET_MS.linkArrival
      }),
      'the connection test did not report the service'
    )
    await options.click('#save')
    const saved = await localized(options, 'saved')
    await options.waitForFunction((text) => document.getElementById('status')?.textContent === text, saved, {
      timeout: BUDGET_MS.linkArrival
    })
    assert.equal(await options.getAttribute('#status', 'class'), 'ok')
    await options.close()
  })

  await t.test('Send page hands the active tab to the LinkGrabber', async () => {
    const popup = await context.newPage()
    await popup.goto(`chrome-extension://${extensionId}/src/popup.html`)
    // The tab the popup sends is the active one in its window, so the page to send is opened
    // from the popup itself: it lands in the same window, in front. Its address is on the
    // service's own origin because that is the one host the extension may read a tab's URL on
    // without a grant; `activeTab` comes only from a click on the toolbar button.
    const id = marker('page')
    const address = `${service.base}/favicon.svg?${id}`
    await popup.evaluate((url) => chrome.tabs.create({ url, active: true }), address)
    await popup.evaluate(async (url) => {
      for (let attempt = 0; attempt < 50; attempt += 1) {
        const [tab] = await chrome.tabs.query({ active: true, currentWindow: true })
        if (tab?.url === url && tab.status === 'complete') return
        await new Promise((done) => setTimeout(done, 100))
      }
      throw new Error(`the page ${url} never became the active tab`)
    }, address)
    // A dispatched click, not a pointer one: the popup is a background tab now, and Playwright's
    // actionability wait needs animation frames a background tab does not get.
    await popup.locator('#page').dispatchEvent('click')
    const sent = await localized(popup, 'sentFromPopup')
    await popup.waitForFunction((text) => document.getElementById('status')?.textContent === text, sent, {
      timeout: BUDGET_MS.linkArrival
    })
    const { value, elapsedMs } = await session.awaitCandidate(id)
    assert.equal(value.url, address)
    console.log(`page in the LinkGrabber after ${elapsedMs} ms`)
  })

  assert.deepEqual(problems, [], 'the extension reported errors')
})
