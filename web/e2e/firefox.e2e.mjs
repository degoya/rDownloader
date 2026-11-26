// The browser extension end to end in Firefox (RD-1480-04): the built Firefox extension in a fresh
// profile, against a fresh service under a name of another machine on a port of its own — the
// address is saved, a site is allowed, its session shared, and Click'n'Load handed over, without
// an agent and beside one. These four are what RD-1480-01 found broken in Firefox alone.
//
//   RD_E2E_SERVER=…/rdownloader RD_E2E_FIREFOX_EXTENSION=artifacts/browser-extensions/firefox \
//     node --test web/e2e/firefox.e2e.mjs
//
// Needs Firefox and geckodriver (RD_E2E_GECKODRIVER, RD_E2E_FIREFOX; by default from PATH); a
// machine without them fails the run rather than skipping it.
//
// **What answers the permission prompt, and what does not.** The prompt is browser chrome; the
// profile sets `extensions.webextOptionalPermissionPrompts=false`, with which Firefox grants what
// the prompt would have asked. It grants nothing that is asked for at the wrong moment: whether a
// request belongs to a person's click is checked before, by Firefox, and the run shows that it is —
// - the probe extension (lib/gesture-probe.mjs) asks in the same Firefox and the same profile
//   without a click, and after an `await` within one, and each time reads Firefox's own refusal;
// - the buttons under test are pressed twice: by a script, which is nobody's click and has to be
//   refused, and by WebDriver, whose click is a trusted pointer event.
// Were the preference to lift the click rule as well, both would be granted and the run fails.
// What the prompt itself says, and a person declining it, stay a live check (docs/development.md).
//
// Names: `network.dns.localDomains` takes `nas.test` and `hoster.test` to this machine.
import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { after, before, test } from 'node:test'

import { CNL_PORT, encrypt, hosterPage, startAgent, startHoster } from './lib/click-and-load.mjs'
import { OUTCOMES, PROBE_ID, REFUSED, writeProbe } from './lib/gesture-probe.mjs'
import { BUDGET_MS, Session, assertPortFree, freePort, marker, startService, stop, within } from './lib/service.mjs'
import { Firefox } from './lib/webdriver.mjs'

const SERVER = process.env.RD_E2E_SERVER
const EXTENSION = process.env.RD_E2E_FIREFOX_EXTENSION && resolve(process.env.RD_E2E_FIREFOX_EXTENSION)
const WORK = process.env.RD_E2E_WORK || mkdtempSync(join(tmpdir(), 'rd-e2e-firefox-'))
const SERVICE_HOST = 'nas.test'
const HOSTER = 'hoster.test'
const MENU_SESSION = 'rdownloader-session'
// An extension's pages live under an id of the profile's choosing; chosen here, so it is known.
const UUIDS = { 'rdownloader@degoya.de': 'c3d1f0f4-6a2e-4c7b-9d5e-1a2b3c4d5e6f', [PROBE_ID]: 'a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d' }
const pageOf = (id, path) => `moz-extension://${UUIDS[id]}/${path}`

let service
let session
let hoster
let agent
let firefox
let address
let hosterAddress

/** The extension's own English text for `key`; the profile asks for English. */
function localized(key, ...values) {
  const entry = JSON.parse(readFileSync(join(EXTENSION, '_locales', 'en', 'messages.json'), 'utf8'))[key]
  assert.ok(entry, `no message ${key}`)
  return entry.message
    .replace(/\$(\w+)\$/g, (whole, name) => entry.placeholders?.[name.toLowerCase()]?.content ?? whole)
    .replace(/\$(\d)/g, (_, index) => values[index - 1] ?? '')
}

/** Saves an address and a token on the options page; `click` is who presses Save. */
async function save(server, token, click) {
  await firefox.open(pageOf('rdownloader@degoya.de', 'src/options.html'))
  await firefox.fill('#server', server)
  await firefox.fill('#token', token)
  await click('#save')
}

/**
 * Opens the popup over the tab `handle`. Firefox opens it from its toolbar, which WebDriver does
 * not reach; as a page it reads the active tab when it loads, so it is reloaded while the other
 * tab is in front. `expected` is the text its site button has to end with.
 */
async function popupOver(handle, expected) {
  const popup = await firefox.newTab()
  await firefox.open(pageOf('rdownloader@degoya.de', 'src/popup.html'))
  await within(BUDGET_MS.linkArrival * 2, `the popup over ${HOSTER}`, async () => {
    await firefox.switchTo(popup)
    await firefox.run('window.setTimeout(() => window.location.reload(), 1000)')
    await firefox.switchTo(handle)
    await new Promise((done) => setTimeout(done, 2_500))
    await firefox.switchTo(popup)
    return (await firefox.displayed('#files-toggle')) && (await firefox.text('#files-toggle')) === expected
  })
}

before(async () => {
  assert.ok(SERVER, 'RD_E2E_SERVER names the rdownloader binary')
  assert.ok(EXTENSION, 'RD_E2E_FIREFOX_EXTENSION names the unpacked Firefox extension')
  await assertPortFree(CNL_PORT)
  service = await startService({ binary: SERVER, port: await freePort(), workDir: join(WORK, 'service') })
  session = new Session(service.base)
  await session.signIn(`e2e-${marker('pw')}`)
  const settings = await session.expect(200, 'GET', '/api/v1/settings')
  await session.expect(200, 'PUT', '/api/v1/settings', { ...settings, allowed_hosts: [SERVICE_HOST] })
  address = `http://${SERVICE_HOST}:${new URL(service.base).port}`
  hoster = await startHoster({ cookie: `rd_e2e_login=${marker('login')}; Path=/; HttpOnly` })
  hosterAddress = `http://${HOSTER}:${hoster.port}`
  hoster.serve('<!doctype html><meta charset="utf-8"><title>A hoster</title><p>Signed in.</p>')

  firefox = await Firefox.start({
    workDir: WORK,
    prefs: {
      'extensions.webextOptionalPermissionPrompts': false,
      'extensions.webextensions.uuids': JSON.stringify(UUIDS),
      'network.dns.localDomains': [SERVICE_HOST, HOSTER].join(','),
      'network.dns.disableIPv6': true,
      'network.trr.mode': 5,
      // Both services are plain http, which is what the run is about.
      'dom.security.https_first': false,
      'dom.security.https_only_mode': false,
      'intl.locale.requested': 'en-US'
    }
  })
  assert.equal(await firefox.install(EXTENSION), 'rdownloader@degoya.de')
})

after(async () => {
  await firefox?.quit()
  await agent?.close()
  await hoster?.close()
  await stop(service?.child)
  console.log(`logs in ${WORK}`)
})

test('the extension in Firefox', { timeout: 300_000 }, async (t) => {
  const bearer = await session.pairCapture('E2E Firefox extension')

  await t.test('Firefox still holds its click rule, with the prompt answered by a preference', async () => {
    assert.equal(await firefox.install(writeProbe(WORK)), PROBE_ID)
    await firefox.open(pageOf(PROBE_ID, 'probe.html'))
    await firefox.click('#ask-direct')
    await firefox.click('#ask-late')
    await firefox.chooseMenuItem('menu-direct')
    await firefox.chooseMenuItem('menu-late')
    const outcomes = {}
    await within(BUDGET_MS.linkArrival, 'the outcomes of the probe', async () => {
      for (const id of OUTCOMES) outcomes[id] = await firefox.text(`#${id}`)
      return OUTCOMES.every((id) => outcomes[id])
    })
    console.log(`click rule: ${JSON.stringify(outcomes)}`)
    assert.equal(outcomes['button-direct'], 'granted', 'a request inside a click is granted')
    assert.equal(outcomes['menu-direct'], 'granted', 'a request inside a menu click is granted')
    for (const id of ['load', 'button-late', 'menu-late']) assert.match(outcomes[id], REFUSED, id)
  })

  await t.test('an address on another machine and port is saved, by a click and not without one', async () => {
    await save(address, bearer, (button) => firefox.scriptedClick(button))
    await firefox.waitText('#status', localized('permissionDenied'))
    await save(address, bearer, (button) => firefox.click(button))
    await firefox.waitText('#status', localized('saved'))
    assert.ok(await firefox.displayed('#server-insecure'), 'the warning about plain http is shown')
    await firefox.click('#test')
    await firefox.waitText('#status', localized('connected', service.version))
  })

  // A popup opened from the toolbar reads its tab's address through `activeTab`. As a page it has
  // none, so the hoster's address is made readable the way the service's was: saved, then replaced.
  // That grants `http://hoster.test/*` alone; what allowing the site asks for is more than that.
  await save(hosterAddress, bearer, (button) => firefox.click(button))
  await firefox.waitText('#status', localized('saved'))
  await save(address, bearer, (button) => firefox.click(button))
  await firefox.waitText('#status', localized('saved'))
  const site = await firefox.newTab()
  await firefox.open(`${hosterAddress}/`)

  await t.test('sharing a site\'s session from the context menu hands its cookie over', async () => {
    await firefox.chooseMenuItem(MENU_SESSION)
    const { value, elapsedMs } = await within(BUDGET_MS.linkArrival, 'the shared session', async () => {
      const profiles = await session.expect(200, 'GET', '/api/v1/auth-profiles')
      return profiles.find((profile) => profile.host === HOSTER) ?? null
    })
    assert.equal(value.method, 'cookies')
    assert.equal(value.enabled, false, 'a shared session waits for approval in the web interface')
    console.log(`session of ${HOSTER} with the service after ${elapsedMs} ms`)
  })

  await t.test('a site is allowed in the popup, by a click and not without one', async () => {
    await popupOver(site, localized('filesAllow', HOSTER))
    await firefox.scriptedClick('#files-toggle')
    await firefox.waitText('#status', localized('filesDenied', HOSTER))
    await firefox.click('#files-toggle')
    await firefox.waitText('#status', localized('filesAllowed', HOSTER))
    await firefox.waitText('#files-toggle', localized('filesRevoke', HOSTER))
  })

  await t.test("Click'n'Load without the agent shows its button and reaches the LinkGrabber", async () => {
    const id = marker('cnl')
    const links = [`http://files.test/${id}.part1.rar`, `http://files.test/${id}.part2.rar`]
    const packageName = `Release ${id}`
    hoster.serve(hosterPage({ crypted: encrypt(links.join('\r\n')), packageName, password: 'secret-pw' }))
    await firefox.switchTo(site)
    await firefox.open(`${hosterAddress}/`)
    await firefox.waitDisplayed('#cnl-button')
    await firefox.click('#cnl-button')
    const { value, elapsedMs } = await session.awaitCandidate(id)
    assert.ok(links.includes(value.url), value.url)
    const packages = await session.expect(200, 'GET', '/api/v1/collector/packages')
    const handed = packages.find((entry) => entry.name === packageName)
    assert.ok(handed, `no package named ${packageName}`)
    assert.equal(handed.password, 'secret-pw')
    console.log(`Click'n'Load in the LinkGrabber after ${elapsedMs} ms`)
  })

  await t.test('with an agent on the port the extension answers nothing and hands nothing over', async () => {
    agent = await startAgent()
    const id = marker('agent')
    hoster.serve(hosterPage({ crypted: encrypt(`http://files.test/${id}.rar`), packageName: `Agent ${id}`, password: 'pw' }))
    await firefox.open(`${hosterAddress}/`)
    await firefox.waitDisplayed('#cnl-button')
    await firefox.click('#cnl-button')
    await within(BUDGET_MS.linkArrival, 'the agent receiving the hand-over', async () => agent.received.length === 1)
    // Give a wrong hand-over the time it would need, then make sure there was none.
    await new Promise((done) => setTimeout(done, 2_000))
    const candidates = await session.expect(200, 'GET', '/api/v1/collector/candidates')
    assert.ok(!candidates.some((candidate) => String(candidate.url).includes(id)), "the extension handed the agent's links over too")
  })
})
