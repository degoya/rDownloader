// Browser extension end to end in Chromium (RD-180-12): the built Chrome extension loaded into a
// fresh profile, paired with a fresh service through its options page, and a page sent from the
// popup, asserted in the LinkGrabber. Since RD-1260-11 also a DLC, once through the extension's
// "Send file to rDownloader…" page and once dropped on the web interface, both against a local
// stand-in for the decryption service.
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

import { dlcContainer, startDlcService } from './lib/dlc.mjs'
import { BUDGET_MS, Session, assertPortFree, marker, startService, stop } from './lib/service.mjs'

const SERVER = process.env.RD_E2E_SERVER
const EXTENSION = process.env.RD_E2E_EXTENSION && resolve(process.env.RD_E2E_EXTENSION)
const WORK = process.env.RD_E2E_WORK || mkdtempSync(join(tmpdir(), 'rd-e2e-extension-'))
const PORT = 8710

let service
let dlcService
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
  // A fresh service opens the setup wizard instead of the LinkGrabber, where a dropped file never
  // reaches the window's drop zone.
  await session.expect(200, 'POST', '/api/v1/setup/complete')
  // DLC import is off by default and needs its service; the stand-in answers on loopback.
  dlcService = await startDlcService()
  const settings = await session.expect(200, 'GET', '/api/v1/settings')
  await session.expect(200, 'PUT', '/api/v1/settings', {
    ...settings,
    dlc_service_enabled: true,
    dlc_service_endpoint: dlcService.endpoint
  })

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
  await dlcService?.close()
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

  await t.test('Send file hands a DLC to the LinkGrabber (RD-1260-11)', async () => {
    const id = marker('dlc')
    const address = `https://example.invalid/${id}.bin`
    const dlc = dlcContainer([address], `E2E ${id}`)
    dlcService.register(dlc)
    const page = await context.newPage()
    await page.goto(`chrome-extension://${extensionId}/src/send-file.html`)
    await page.setInputFiles('#files', { name: `${id}.dlc`, mimeType: 'application/octet-stream', buffer: Buffer.from(dlc.container) })
    const sent = await localized(page, 'sendFileSent', ['1'])
    await page.waitForFunction((text) => document.getElementById('status')?.textContent === text, sent, {
      timeout: BUDGET_MS.linkArrival
    })
    const { value, elapsedMs } = await session.awaitCandidate(id)
    assert.equal(value.url, address)
    console.log(`DLC from the send-file page in the LinkGrabber after ${elapsedMs} ms`)
    await page.close()
  })

  await t.test('a DLC dropped on the web interface is imported (RD-1260-11)', async () => {
    const id = marker('drop')
    const address = `https://example.invalid/${id}.bin`
    const dlc = dlcContainer([address], `E2E ${id}`)
    dlcService.register(dlc)
    // The web interface signed in as the session above: its cookies, handed to the browser.
    await context.addCookies(session.cookie.split('; ').filter(Boolean).map((pair) => {
      const at = pair.indexOf('=')
      return { name: pair.slice(0, at), value: pair.slice(at + 1), url: service.base }
    }))
    const page = await context.newPage()
    await page.goto(`${service.base}/linkgrabber`)
    await page.waitForLoadState('networkidle')
    const submit = page.locator('button[type="submit"][form="nzb-import-form"]')
    // The window's drop zone exists once the layout is mounted; a drop before it is lost, so it
    // is repeated until the import dialog opens with the file.
    for (let attempt = 0; attempt < 10 && !(await submit.isVisible()); attempt += 1) {
      await page.evaluate(([name, text]) => {
        const transfer = new DataTransfer()
        transfer.items.add(new File([text], name))
        for (const type of ['dragenter', 'dragover', 'drop']) {
          window.dispatchEvent(new DragEvent(type, { dataTransfer: transfer, bubbles: true, cancelable: true }))
        }
      }, [`${id}.dlc`, dlc.container])
      await submit.waitFor({ state: 'visible', timeout: 1_000 }).catch(() => {})
    }
    await submit.click()
    const { value, elapsedMs } = await session.awaitCandidate(id)
    assert.equal(value.url, address)
    console.log(`DLC dropped on the web interface in the LinkGrabber after ${elapsedMs} ms`)
    await page.close()
  })

  assert.deepEqual(problems, [], 'the extension reported errors')
})
