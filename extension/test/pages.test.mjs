// The popup, the options and the send-file page, opened and clicked in both browsers (RD-1480-02).
//
// Their decisions have suites of their own; what was untested is the wiring between a button and
// them, and that is where the save of the options page was lost in Firefox (RD-1480-01).

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

import { SITES_KEY, sitePattern } from '../src/files.js'

import { fakeBrowser, loadIn, settle } from './fake-browser.mjs'
import { openPage } from './fake-page.mjs'
import { WIDGET } from './fakes.mjs'

const PAIRED = { server: 'http://127.0.0.1:8710', token: 'capture-token' }
const HOST = 'hoster.test'
const HANDOVER = { id: 'h1', scope: `https://${HOST}/`, host: HOST, account_label: 'Main' }

const json = (payload, status = 200) => ({ ok: status < 400, status, json: async () => payload })

/** The service with one waiting captcha and one waiting session request. */
async function service(url) {
  if (url.includes('/capture/captchas?')) return json([WIDGET])
  if (url.endsWith('/capture/browser-sessions')) return json([HANDOVER])
  if (url.endsWith('/capture/ping')) return json({ version: '1.48.0', capture_version: 2 })
  return json({ candidates: [{}, {}] }, 201)
}

async function open(target, name, { page, ...options } = {}) {
  const browser = fakeBrowser(target, { local: PAIRED, ...options })
  const opened = await openPage(browser, name, page)
  await settle()
  return { browser, page: opened, text: browser.text }
}

for (const target of ['firefox', 'chrome']) {
  test(`${target}: saving an address asks within the click, stores it and reaches the service by its grant`, async () => {
    for (const [typed, saved] of [['192.168.178.123:8710', 'http://192.168.178.123:8710'], ['https://dl.example.com/', 'https://dl.example.com']]) {
      const { browser, page, text } = await open(target, 'options', { local: {}, respond: service })
      page.type('server', typed)
      page.byId('token').value = ' abc '
      page.type('intercept-mode', 'only', 'change')
      page.type('intercept-types', '*.DLC nzb')
      page.byId('click-and-load').checked = false
      page.click(page.byId('save'))
      assert.equal(browser.prompts.length, 1, 'the prompt is there before the click handler returned')
      await settle()
      assert.deepEqual(page.status(), [text('saved'), 'ok'])
      assert.deepEqual(
        { ...browser.stored },
        { server: saved, token: 'abc', interceptDownloads: true, interceptMode: 'only', interceptTypes: ['.dlc', '.nzb'], clickAndLoad: false }
      )
      assert.equal(page.byId('server').value, saved)
      assert.equal(page.byId('intercept-types').value, '.dlc .nzb')

      page.click(page.byId('test'))
      await settle()
      assert.deepEqual(page.status(), [text('connected', ['1.48.0']), 'ok'])
      assert.deepEqual(browser.fetches.map(({ url, hostAccess }) => [url, hostAccess]), [[`${saved}/api/v1/capture/ping`, true]], 'the grant covers the server')
    }
  })

  test(`${target}: a declined permission stores nothing and says which refusal it was`, async () => {
    const { browser, page, text } = await open(target, 'options', { local: {}, answer: () => false })
    page.type('server', 'http://nas.local:8710')
    page.click(page.byId('save'))
    await settle()
    assert.deepEqual(page.status(), [text('permissionDenied'), 'error'])
    assert.deepEqual({ ...browser.stored }, {})
    page.type('server', '[::1')
    page.click(page.byId('save'))
    page.click(page.byId('test'))
    await settle()
    assert.deepEqual(page.status(), [text('serverInvalid'), 'error'])
    assert.equal(browser.prompts.length, 1, 'nothing is asked for an address that is not one')
    assert.deepEqual(browser.fetches, [])
  })

  test(`${target}: the options show what is stored, warn about plain http and take a site back`, async () => {
    const local = { server: 'http://nas.local:8710', token: 'abc', interceptDownloads: false, interceptMode: 'only', interceptTypes: [], clickAndLoad: false, [SITES_KEY]: [HOST] }
    const { browser, page, text } = await open(target, 'options', { local, grants: ['cookies', sitePattern(HOST)] })
    assert.equal(page.byId('server').value, 'http://nas.local:8710')
    assert.equal(page.byId('intercept').checked, false)
    assert.equal(page.byId('click-and-load').checked, false)
    assert.equal(page.byId('server-insecure').hidden, false)
    assert.equal(page.byId('intercept-types-notice').textContent, text('optionsInterceptTypesEmpty'))
    page.type('server', 'https://nas.local')
    assert.equal(page.byId('server-insecure').hidden, true)
    page.type('intercept-mode', 'all', 'change')
    assert.equal(page.byId('intercept-types').disabled, true)
    assert.equal(page.byId('intercept-types-notice').hidden, true)

    page.click(page.button('file-sites', text('optionsFilesRevoke')))
    await settle()
    assert.deepEqual(page.status(), [text('filesRevoked', [HOST]), 'ok'])
    assert.deepEqual(browser.stored[SITES_KEY], [])
    assert.equal(page.byId('file-sites-none').hidden, false)
    assert.ok(!browser.held().includes(sitePattern(HOST)))
  })

  test(`${target}: the popup allows the open site within the click and takes it back`, async () => {
    const { browser, page, text } = await open(target, 'popup')
    const toggle = page.byId('files-toggle')
    assert.equal(page.byId('files').hidden, false)
    assert.equal(toggle.textContent, text('filesAllow', [HOST]))
    page.click(toggle)
    assert.equal(browser.prompts.length, 1, 'the prompt is there before the click handler returned')
    await settle()
    assert.deepEqual(page.status(), [text('filesAllowed', [HOST]), 'ok'])
    assert.deepEqual(browser.stored[SITES_KEY], [HOST])
    assert.equal(toggle.textContent, text('filesRevoke', [HOST]))
    page.click(toggle)
    await settle()
    assert.deepEqual(page.status(), [text('filesRevoked', [HOST]), 'ok'])
    assert.deepEqual(browser.stored[SITES_KEY], [])
  })

  test(`${target}: the popup's own buttons send the page and the clipboard and open the rest`, async () => {
    let clipboard = () => 'https://hoster.test/a https://hoster.test/b'
    const { browser, page, text } = await open(target, 'popup', { page: { clipboard: () => clipboard() } })
    const sent = []
    let answer = { ok: true, links: 1 }
    browser.api.runtime.onMessage.addListener((request, _sender, respond) => {
      if (request.type !== 'rdownloader:send') return false
      sent.push(request)
      respond(answer)
      return true
    })
    page.click(page.byId('page'))
    await settle()
    assert.deepEqual(sent.at(-1), { type: 'rdownloader:send', text: 'https://hoster.test/folder', packageName: 'A folder' })
    assert.deepEqual(page.status(), [text('sentLinksOne', ['1']), 'ok'])
    answer = { ok: true, links: 2 }
    page.click(page.byId('clipboard'))
    await settle()
    assert.deepEqual(sent.at(-1), { type: 'rdownloader:send', text: 'https://hoster.test/a https://hoster.test/b', sourceLabel: 'Clipboard' })
    assert.deepEqual(page.status(), [text('sentLinks', ['2']), 'ok'])
    clipboard = () => { throw new Error('denied') }
    page.click(page.byId('clipboard'))
    await settle()
    assert.deepEqual(page.status(), [text('clipboardDenied'), 'error'])
    browser.api.tabs.query = async () => [{ url: 'about:addons' }]
    page.click(page.byId('page'))
    await settle()
    assert.deepEqual(page.status(), [text('noPageUrl'), 'error'])
    assert.equal(sent.length, 2)

    page.byId('intercept').checked = false
    page.byId('intercept').dispatch('change')
    await settle()
    assert.equal(browser.stored.interceptDownloads, false)
    page.click(page.byId('options'))
    assert.ok(browser.calls.some(([name]) => name === 'openOptionsPage'))
    page.click(page.byId('send-file'))
    await settle()
    assert.deepEqual(browser.calls.findLast(([name]) => name === 'createTab')[1], { url: browser.api.runtime.getURL('src/send-file.html') })
    assert.equal(page.closed, true)
    assert.equal(page.byId('version').textContent, `v${browser.manifest.version}`)
  })

  test(`${target}: the popup lists what waits, and each consent is asked within its click`, async () => {
    // The background answers the popup's messages here, so both halves run in this browser.
    const browser = fakeBrowser(target, { local: PAIRED, cookies: [{ name: 's', value: '1', domain: `.${HOST}`, path: '/', secure: true }], respond: service })
    await loadIn(browser, 'background.js')
    const page = await openPage(browser, 'popup')
    await settle()
    const { text } = browser
    assert.equal(page.byId('captchas').hidden, false)
    assert.equal(page.byId('handovers').hidden, false)

    const before = browser.prompts.length
    page.click(page.button('handover-list', text('handoverAllow')))
    assert.equal(browser.prompts.length, before + 1, 'the handover asks before its handler returned')
    await settle()
    assert.deepEqual(page.status(), [text('handoverDone', [HOST]), 'ok'])
    assert.ok(!browser.held().includes('cookies'), 'and gives the grant back')

    page.click(page.button('captcha-list', text('captchaAnswer')))
    assert.equal(browser.prompts.length, before + 2, 'the captcha asks before its handler returned')
    await settle()
    assert.deepEqual(page.status(), [text('captchaOpened'), 'ok'])
    page.click(page.button('captcha-list', text('captchaDecline')))
    await settle()
    assert.deepEqual(page.status(), [text('captchaSkipped', ['ddownload.com']), 'ok'])
    page.click(page.button('handover-list', text('handoverDecline')))
    await settle()
    assert.deepEqual(page.status(), [text('handoverDeclined', [HOST]), 'ok'])
  })
}

test('the send-file page hands a chosen file over, refuses an unknown one, and asks to be paired first', async () => {
  const { browser, page, text } = await open('firefox', 'send-file', { respond: service })
  const input = page.byId('files')
  const choose = async (...files) => {
    input.files = files
    input.dispatch('change')
    await settle()
  }
  await choose(new File(['x'], 'a.dlc'))
  assert.deepEqual(page.status(), [text('sendFileSent', ['1']), 'ok'])
  assert.deepEqual(browser.notices(), [text('sendFileSent', ['1'])])
  await choose(new File(['x'], 'a.txt'))
  assert.deepEqual(page.status(), [text('sendFileUnknown', ['a.txt']), 'error'])
  page.byId('drop').dispatch('drop', { preventDefault: () => {}, dataTransfer: { files: [new File(['x'], 'b.nzb')] } })
  await settle()
  assert.equal(browser.fetches.length, 2)
  browser.stored.token = ''
  await choose(new File(['x'], 'c.nzb'))
  assert.deepEqual(page.status(), [text('notConfigured'), 'error'])
  assert.equal(browser.fetches.length, 2)
})

test('every page names its language, its title and its regions, and says its status aloud', async () => {
  // WCAG 2.2: 3.1.1, 2.4.2, 1.3.1, 4.1.3 (A11Y-6, A11Y-7, I18N-4).
  for (const [name, title] of [['popup', 'extName'], ['options', 'extName'], ['send-file', 'sendFileTitle']]) {
    for (const language of ['de', 'en', 'es', 'fr']) {
      const { page, text } = await open('firefox', name, { language })
      assert.equal(page.root.lang, language, `${name} in ${language}`)
      assert.equal(page.title(), text(title))
      for (const element of page.document.find((candidate) => candidate.dataset.i18n)) assert.ok(element.textContent, element.dataset.i18n)
    }
    const { page } = await open('chrome', name)
    assert.equal(page.all('title').length, 1, `${name} has a title before its script runs`)
    assert.equal(page.root.attributes.lang, 'en', 'and a language')
    assert.equal(page.all('main').length, 1)
    assert.equal(page.all('h1').length, 1)
    assert.deepEqual([...page.all('h3'), ...page.all('h4')], [], 'no heading level is skipped')
    assert.equal(page.byId('status').attributes.role, 'status')
    assert.ok(page.all('main')[0].find((element) => element.attributes.id === 'status').length, 'the status is inside the main region')
  }
})

/** The contrast of two `#rrggbb` colours, as WCAG computes it. */
function contrast(one, other) {
  const luminance = (colour) => {
    const [r, g, b] = [1, 3, 5].map((at) => Number.parseInt(colour.slice(at, at + 2), 16) / 255).map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4))
    return 0.2126 * r + 0.7152 * g + 0.0722 * b
  }
  const [light, dark] = [luminance(one), luminance(other)].sort((a, b) => b - a)
  return (light + 0.05) / (dark + 0.05)
}

test('buttons, links, hints and the status reach 4.5:1 in both themes', () => {
  // WCAG 1.4.3 (A11Y-4, A11Y-5). Read from the pages: the last rule of a theme for a selector.
  for (const name of ['popup', 'options', 'send-file']) {
    const style = /<style>([\s\S]*?)<\/style>/.exec(readFileSync(new URL(`../src/${name}.html`, import.meta.url), 'utf8'))[1]
    const DARK = /@media \(prefers-color-scheme: dark\) \{((?:[^{}]*\{[^{}]*\})*)\s*\}/g
    // In place, so the order of the rules decides here as it does in the browser.
    const dark = style.replace(DARK, '$1')
    const light = style.replace(DARK, '')
    const colourOf = (css, selector, property) => {
      const rules = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter(([, names]) => names.split(',').some((one) => one.trim() === selector))
      return rules.flatMap(([, , body]) => [...body.matchAll(new RegExp(`(?:^|;|\\s)${property}:[^;#]*(#[0-9a-f]{3,6})`, 'g'))].map((match) => match[1])).at(-1) ?? null
    }
    const full = (colour) => (colour.length === 4 ? `#${[...colour.slice(1)].map((c) => c + c).join('')}` : colour)
    for (const [theme, css] of [['light', light], ['dark', dark]]) {
      const ground = colourOf(css, 'body', 'background')
      for (const selector of ['.ok', '.error', 'p.hint', 'p.warning', '#drop span']) {
        const colour = colourOf(css, selector, 'color')
        if (colour) assert.ok(contrast(full(colour), full(ground)) >= 4.5, `${name} ${theme} ${selector}: ${colour} on ${ground}`)
      }
      for (const selector of ['button', 'button.secondary']) {
        const colour = colourOf(css, selector, 'background')
        if (colour) assert.ok(contrast('#ffffff', full(colour)) >= 4.5, `${name} ${theme} ${selector}: white on ${colour}`)
      }
      // 1.4.11: a field is seen by its edge, against the page and against its own inside.
      for (const [selector, property] of [['input', 'border'], ['#drop', 'border'], ['#drop.active', 'border-color']]) {
        const edge = colourOf(css, selector, property)
        const inside = colourOf(css, selector, 'background') ?? (selector === 'input' ? '#ffffff' : ground)
        if (edge) for (const beside of [ground, inside]) assert.ok(contrast(full(edge), full(beside)) >= 3, `${name} ${theme} ${selector}: ${edge} beside ${beside}`)
      }
    }
  }
  // 2.4.7 (A11Y-8): the invisible file field shows its focus on the area it belongs to.
  assert.match(readFileSync(new URL('../src/send-file.html', import.meta.url), 'utf8'), /#drop:has\(\+ #files:focus-visible\) \{ outline: 2px solid/)
})
