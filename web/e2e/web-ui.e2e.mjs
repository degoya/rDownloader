// The web interface in a real browser (RD-1480-25): a link is added in the LinkGrabber, checked,
// put into the queue and downloaded from a file host on this machine, all by clicks in Chromium,
// against a fresh service. Each page on the way is read by axe in light and in dark, contrast
// included: the component tests run in jsdom, where nothing is painted.
//
//   RD_E2E_SERVER=…/rdownloader node --test web/e2e/web-ui.e2e.mjs
//
// The interface is the one the binary embeds, so it has to be built before the binary is.
import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync, readdirSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { after, before, test } from 'node:test'
import { fileURLToPath } from 'node:url'

import { chromium } from 'playwright'

import { axeViolations, settled } from './lib/axe.mjs'
import { BUDGET_MS, Session, freePort, marker, startFileHost, startService, stop, within } from './lib/service.mjs'

const SERVER = process.env.RD_E2E_SERVER
const WORK = process.env.RD_E2E_WORK || mkdtempSync(join(tmpdir(), 'rd-e2e-web-'))
const LOCALES = join(dirname(fileURLToPath(import.meta.url)), '..', 'src', 'locales', 'en')
const PASSWORD = `e2e-${marker('pw')}`
const MODES = ['light', 'dark']

let service
let session
let host
let browser
let page
const problems = []

/** A text of the English catalogue, `<file>.<key>…`, so a reworded label fails nothing here. */
function text(key) {
  const [file, ...path] = key.split('.')
  const catalogue = JSON.parse(readFileSync(join(LOCALES, `${file}.json`), 'utf8'))
  const value = path.reduce((node, part) => node?.[part], catalogue)
  assert.equal(typeof value, 'string', `no text ${key}`)
  return value
}

/** The downloaded file of that name, wherever the package put it; null while there is none. */
function downloaded(name) {
  const root = join(WORK, 'service', 'downloads')
  const found = readdirSync(root, { recursive: true }).map(String).find((entry) => entry.split(/[\\/]/).at(-1) === name)
  return found ? readFileSync(join(root, found)) : null
}

/** Reads the page with axe in both colour modes; the interface follows the browser's preference. */
async function assertAccessible(what) {
  const found = []
  for (const mode of MODES) {
    await page.emulateMedia({ colorScheme: mode })
    await page.waitForFunction((dark) => document.documentElement.classList.contains('dark') === dark, mode === 'dark')
    await settled(page)
    for (const line of await axeViolations(page)) found.push(`${what}, ${mode}: ${line}`)
  }
  assert.deepEqual(found, [])
}

before(async () => {
  assert.ok(SERVER, 'RD_E2E_SERVER names the rdownloader binary')
  service = await startService({ binary: SERVER, port: await freePort(), workDir: join(WORK, 'service') })
  console.log(`service ${service.version} on ${service.base} after ${service.startMs} ms`)
  session = new Session(service.base)
  await session.signIn(PASSWORD)
  // A fresh service opens the setup wizard instead of the views this run is about.
  await session.expect(200, 'POST', '/api/v1/setup/complete')
  host = await startFileHost()
  browser = await chromium.launch({ headless: process.env.RD_E2E_HEADED !== '1' })
  const context = await browser.newContext({ locale: 'en-US', reducedMotion: 'reduce', viewport: { width: 1280, height: 900 } })
  page = await context.newPage()
  page.on('pageerror', (error) => problems.push(`pageerror: ${error.message}`))
  page.on('console', (entry) => {
    // The sign-in screen asks for a session it does not have yet; that 401 is its answer.
    if (entry.type() === 'error' && !/status of 401/.test(entry.text())) problems.push(`console: ${entry.text()}`)
  })
})

after(async () => {
  await browser?.close()
  await host?.close()
  await stop(service?.child)
  console.log(`logs in ${WORK}`)
})

test('a link goes from the LinkGrabber into the queue and onto the disk', { timeout: 180_000 }, async (t) => {
  const name = `${marker('web')}.bin`
  const address = `${host.origin}/${name}`

  await t.test('the password signs in', async () => {
    await page.goto(service.base)
    const field = page.locator('input[type="password"]').first()
    await field.waitFor({ state: 'visible', timeout: BUDGET_MS.linkArrival })
    await assertAccessible('the sign-in screen')
    await field.fill(PASSWORD)
    await field.press('Enter')
    await field.waitFor({ state: 'detached', timeout: BUDGET_MS.linkArrival })
  })

  await t.test('a link added in the LinkGrabber is checked and online', async () => {
    await page.goto(`${service.base}/linkgrabber`)
    await page.getByRole('button', { name: text('linkgrabber.actions.add_links') }).click()
    const field = page.locator('#collector-intake-form textarea')
    await field.waitFor({ state: 'visible' })
    await assertAccessible('the dialog for new links')
    await field.fill(address)
    await page.locator('button[form="collector-intake-form"]').click()
    const row = page.getByText(name, { exact: true })
    await row.first().waitFor({ state: 'visible', timeout: BUDGET_MS.linkArrival })
    await within(BUDGET_MS.linkArrival, 'the link check', async () => {
      const candidates = await session.expect(200, 'GET', '/api/v1/collector/candidates')
      return candidates.find((candidate) => candidate.url === address)?.state === 'online'
    })
    await assertAccessible('the LinkGrabber with a link')
  })

  await t.test('enqueued, it is downloaded and shown as completed', async () => {
    await page.getByRole('button', { name: text('linkgrabber.actions.enqueue_all') }).click()
    const { elapsedMs } = await within(BUDGET_MS.linkArrival, 'the download', async () => {
      const downloads = await session.expect(200, 'GET', '/api/v1/downloads')
      return downloads.find((download) => download.file_name === name)?.state === 'completed'
    })
    console.log(`downloaded after ${elapsedMs} ms`)
    const expected = Buffer.from(await (await fetch(address)).arrayBuffer())
    assert.ok(downloaded(name)?.equals(expected), 'the file on the disk is the one the host serves')

    await page.goto(`${service.base}/downloads`)
    await page.getByTestId('packages-open-toggle').click()
    await page.getByText(name, { exact: true }).first().waitFor({ state: 'visible', timeout: BUDGET_MS.linkArrival })
    await page.locator('.queue-cell-state', { hasText: text('common.states.completed') }).first().waitFor({ state: 'visible' })
    await assertAccessible('the queue with a finished download')
  })

  await t.test('the settings', async () => {
    await page.goto(`${service.base}/settings`)
    await page.locator('#main-content h1, #main-content h2').first().waitFor({ state: 'visible' })
    await assertAccessible('the settings')
  })

  assert.deepEqual(problems, [], 'the interface reported errors')
})
