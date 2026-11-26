// Firefox through geckodriver, spoken to as WebDriver over HTTP (RD-1480-04). Playwright cannot
// load a Firefox extension; geckodriver installs one for the session (`moz/addon/install`,
// temporary), and the runner images carry it beside Firefox, so nothing is added to the project.
// Node built-ins only.
import { execFileSync } from 'node:child_process'
import { mkdirSync } from 'node:fs'
import { join } from 'node:path'

import { freePort, launch, stop, tail, within } from './service.mjs'

/** The key WebDriver names an element by in its answers. */
const ELEMENT = 'element-6066-11e4-a52e-4f735466cecf'

/** Firefox's first start of a fresh profile, on a runner that has not loaded the binary yet. */
const START_MS = 60_000
const STEP_MS = 15_000

export class Firefox {
  constructor(base, session, child, capabilities) {
    this.base = base
    this.session = session
    this.child = child
    this.capabilities = capabilities
  }

  /**
   * Starts geckodriver and one Firefox with a throwaway profile. `prefs` go into that profile;
   * `--allow-system-access` is what lets a step reach the browser's own window, which is where a
   * context menu lives.
   */
  static async start({ workDir, prefs }) {
    const geckodriver = process.env.RD_E2E_GECKODRIVER || 'geckodriver'
    const version = execFileSync(geckodriver, ['--version'], { encoding: 'utf8' }).split('\n')[0]
    mkdirSync(workDir, { recursive: true })
    const logPath = join(workDir, 'geckodriver.log')
    const port = await freePort()
    const child = launch(
      geckodriver,
      ['--host', '127.0.0.1', '--port', String(port), '--log', 'debug', '--allow-system-access'],
      { cwd: workDir, logPath }
    )
    const base = `http://127.0.0.1:${port}`
    try {
      await within(STEP_MS, 'geckodriver', async () => {
        if (child.launchError) throw child.launchError
        return (await (await fetch(`${base}/status`)).json()).value?.ready
      })
      const options = { args: process.env.RD_E2E_HEADED === '1' ? [] : ['-headless'], prefs }
      if (process.env.RD_E2E_FIREFOX) options.binary = process.env.RD_E2E_FIREFOX
      const created = await send(base, 'POST', '/session', {
        capabilities: { alwaysMatch: { browserName: 'firefox', 'moz:firefoxOptions': options } }
      }, START_MS)
      console.log(`Firefox ${created.capabilities.browserVersion} through ${version}`)
      return new Firefox(base, created.sessionId, child, created.capabilities)
    } catch (error) {
      await stop(child)
      throw new Error(`Firefox could not be started: ${error.message}\n${tail(logPath)}`, { cause: error })
    }
  }

  /** Ends the session, and with it Firefox and its profile. */
  async quit() {
    await this.call('DELETE', '').catch(() => {})
    await stop(this.child)
  }

  call(method, path, body) {
    return send(this.base, method, `/session/${this.session}${path}`, body)
  }

  /** Loads an unpacked extension for this session, as about:debugging does. */
  install(path) {
    return this.call('POST', '/moz/addon/install', { path, temporary: true })
  }

  open(url) {
    return this.call('POST', '/url', { url })
  }

  async newTab() {
    const { handle } = await this.call('POST', '/window/new', { type: 'tab' })
    await this.switchTo(handle)
    return handle
  }

  /** Addresses the tab and brings it to the front. */
  switchTo(handle) {
    return this.call('POST', '/window', { handle })
  }

  async find(selector) {
    return (await this.call('POST', '/element', { using: 'css selector', value: selector }))[ELEMENT]
  }

  async text(selector) {
    return this.call('GET', `/element/${await this.find(selector)}/text`)
  }

  async displayed(selector) {
    return this.call('GET', `/element/${await this.find(selector)}/displayed`)
  }

  /** A real click: a trusted pointer event at the element, which Firefox takes as user input. */
  async click(selector) {
    await this.call('POST', `/element/${await this.find(selector)}/click`, {})
  }

  async fill(selector, text) {
    const id = await this.find(selector)
    await this.call('POST', `/element/${id}/clear`, {})
    await this.call('POST', `/element/${id}/value`, { text })
  }

  run(script, ...args) {
    return this.call('POST', '/execute/sync', { script, args })
  }

  /** A click made by a script: the same handler runs, but nobody clicked. */
  scriptedClick(selector) {
    return this.run('document.querySelector(arguments[0]).click()', selector)
  }

  /** Waits until the element shows exactly `expected`. */
  async waitText(selector, expected) {
    let seen = null
    await within(STEP_MS, `"${expected}" in ${selector}`, async () => {
      seen = await this.text(selector)
      return seen === expected
    }).catch((error) => {
      throw new Error(`${error.message}; it shows "${seen}"`, { cause: error })
    })
  }

  waitDisplayed(selector) {
    return within(STEP_MS, selector, () => this.displayed(selector))
  }

  /** Runs `script` in the browser's own window rather than the page. */
  async inBrowser(script, ...args) {
    await this.call('POST', '/moz/context', { context: 'chrome' })
    try {
      return await this.run(script, ...args)
    } finally {
      await this.call('POST', '/moz/context', { context: 'content' })
    }
  }

  /**
   * Opens the page's context menu with a right click and chooses an extension's item. Firefox
   * builds the items when the menu opens, under the id `<extension>-menuitem-_<item id>`; an
   * extension that has only just been installed may not have registered them yet.
   */
  async chooseMenuItem(item) {
    await within(STEP_MS, `the context menu item ${item}`, async () => {
      await this.call('POST', '/actions', {
        actions: [{
          type: 'pointer',
          id: 'mouse',
          actions: [
            // The margin of the page: a frame or a link there would be a menu of its own.
            { type: 'pointerMove', origin: 'viewport', x: 4, y: 4 },
            { type: 'pointerDown', button: 2 },
            { type: 'pointerUp', button: 2 }
          ]
        }]
      })
      return within(3_000, 'the open menu', () => this.inBrowser(`
        const menu = document.getElementById('contentAreaContextMenu')
        if (menu.state !== 'open') return false
        const item = menu.querySelector('menuitem[id$="-menuitem-_' + arguments[0] + '"]')
        if (item) item.doCommand()
        menu.hidePopup()
        return Boolean(item)
      `, item))
    })
  }
}

async function send(base, method, path, body, timeoutMs = START_MS) {
  const response = await fetch(`${base}${path}`, {
    method,
    headers: body === undefined ? {} : { 'content-type': 'application/json; charset=utf-8' },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(timeoutMs)
  })
  const { value } = await response.json()
  if (!response.ok) throw new Error(`${method} ${path}: ${value?.error}: ${value?.message}`)
  return value
}
