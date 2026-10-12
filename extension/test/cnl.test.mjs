// Click'n'Load without the desktop agent (RD-1260-10): what a page's request to the agent's port
// becomes, who answers it, and that `jk` is never run in the browser.

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

import { CNL_URL_PATTERNS, MAX_HANDOVER_BYTES, RESOURCES, RULE_IDS, cnlPayload, cnlRoute, handoverOutcome, initiatorOf, requestFields, sessionRules } from '../src/cnl.js'
import { PROBE_ALARM, createClickAndLoad, submitClickAndLoad } from '../src/cnl-handover.js'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const CONFIG = { server: 'http://127.0.0.1:8710', token: 'capture-token', clickAndLoad: true }
const PAGE = 'https://hoster.test/folder/1'
const JK = "function f(){ globalThis.__cnlRan = true; return 'abababababababababababababababab'; }"

test('only the agent\'s four paths on its two addresses are Click\'n\'Load', () => {
  assert.equal(cnlRoute('http://127.0.0.1:9666/jdcheck.js'), 'check')
  assert.equal(cnlRoute('http://localhost:9666/flash'), 'flash')
  assert.equal(cnlRoute('http://127.0.0.1:9666/flash/add?urls=x'), 'add')
  assert.equal(cnlRoute('http://localhost:9666/flash/addcrypted2'), 'addcrypted2')
  for (const other of [
    'http://127.0.0.1:9667/flash/add',
    'https://127.0.0.1:9666/flash/add',
    'http://127.0.0.1:9666/crossdomain.xml',
    'http://127.0.0.1:9666/rdownloader/nzb',
    'http://evil.test:9666/flash/add',
    'not an address'
  ]) {
    assert.equal(cnlRoute(other), null, other)
  }
  assert.deepEqual(CNL_URL_PATTERNS, ['http://127.0.0.1:9666/*', 'http://localhost:9666/*'])
})

test('the fields come from the form body over the query, known names only', () => {
  const posted = requestFields({
    url: 'http://127.0.0.1:9666/flash/addcrypted2?package=FromQuery',
    requestBody: { formData: { crypted: ['abc', 'second'], jk: [JK], package: ['Release'], cookie: ['x'] } }
  })
  assert.deepEqual(posted, { crypted: 'abc', jk: JK, package: 'Release' })
  const raw = requestFields({
    url: 'http://127.0.0.1:9666/flash/add',
    requestBody: { raw: [{ bytes: new TextEncoder().encode('urls=https%3A%2F%2Fa.test%2Fx&passwords=pw').buffer }] }
  })
  assert.deepEqual(raw, { urls: 'https://a.test/x', passwords: 'pw' })
  assert.deepEqual(requestFields({ url: 'http://127.0.0.1:9666/flash/add?urls=https://a.test/y' }), { urls: 'https://a.test/y' })
  const oversized = requestFields({
    url: 'http://127.0.0.1:9666/flash/addcrypted2',
    requestBody: { formData: { crypted: ['A'.repeat(MAX_HANDOVER_BYTES + 1)] } }
  })
  assert.equal(oversized, null, 'a request over the service\'s limit is not passed on')
})

test('jk travels as the string the page sent and is never run here', () => {
  delete globalThis.__cnlRan
  const body = cnlPayload('addcrypted2', { crypted: 'abc', jk: JK, passwords: 'pw', package: 'P', source: 'javascript:alert(1)' }, PAGE)
  assert.deepEqual(body, { crypted: 'abc', jk: JK, passwords: 'pw', package: 'P', source: PAGE })
  assert.equal(globalThis.__cnlRan, undefined, 'the script ran in the browser')
  // `key` is the other spelling of the same field.
  assert.equal(cnlPayload('addcrypted2', { crypted: 'abc', key: 'ab'.repeat(16) }, PAGE).jk, 'ab'.repeat(16))
  // Neither module that touches the fields can evaluate anything.
  for (const name of ['cnl.js', 'cnl-handover.js']) {
    const source = readFileSync(join(root, 'src', name), 'utf8')
    assert.ok(!/\beval\s*\(|\bFunction\s*\(|new\s+Function\b/.test(source), `${name} can evaluate a script`)
  }
})

test('a request with nothing to hand over builds no body', () => {
  assert.equal(cnlPayload('addcrypted2', { jk: JK }, PAGE), null)
  assert.equal(cnlPayload('add', { package: 'P' }, PAGE), null)
  assert.equal(cnlPayload('check', { urls: 'x' }, PAGE), null)
  assert.deepEqual(cnlPayload('add', { url: 'https://a.test/x', source: 'https://other.test/' }, PAGE), {
    urls: 'https://a.test/x',
    source: 'https://other.test/'
  })
})

test('the initiator is read in either browser\'s spelling', () => {
  assert.equal(initiatorOf({ initiator: 'https://hoster.test' }), 'https://hoster.test')
  assert.equal(initiatorOf({ originUrl: PAGE }), PAGE)
  assert.equal(initiatorOf({ documentUrl: PAGE }), PAGE)
  assert.equal(initiatorOf({}), null)
})

test('Chrome\'s rules exist only for allowed sites and answer with the extension\'s resources', () => {
  assert.deepEqual(sessionRules([]), [], 'an empty initiator list would mean every page')
  const rules = sessionRules(['hoster.test', 'hoster.test', 'other.test'])
  assert.deepEqual(rules.map((rule) => rule.id), RULE_IDS)
  for (const rule of rules) {
    assert.deepEqual(rule.condition.initiatorDomains, ['hoster.test', 'other.test'])
    assert.equal(rule.action.type, 'redirect')
  }
  assert.deepEqual(rules.map((rule) => rule.action.redirect.extensionPath), [`/${RESOURCES.check}`, `/${RESOURCES.flash}`, `/${RESOURCES.done}`])
  const matches = (rule, url) => new RegExp(rule.condition.regexFilter).test(url)
  assert.ok(matches(rules[0], 'http://127.0.0.1:9666/jdcheck.js'))
  assert.ok(matches(rules[0], 'http://localhost:9666/jdcheck.js?x=1'))
  assert.ok(!matches(rules[0], 'http://127.0.0.1:9667/jdcheck.js'))
  assert.ok(!matches(rules[0], 'http://127x0x0x1:9666/jdcheck.js'))
  assert.ok(matches(rules[1], 'http://127.0.0.1:9666/flash'))
  assert.ok(!matches(rules[1], 'http://127.0.0.1:9666/flash/add'))
  assert.ok(matches(rules[2], 'http://127.0.0.1:9666/flash/add'))
  assert.ok(matches(rules[2], 'http://127.0.0.1:9666/flash/addcrypted2'))
  assert.ok(!matches(rules[2], 'http://127.0.0.1:9666/flash/addsomething'))
})

test('the resources answer what the agent answers', () => {
  assert.equal(readFileSync(join(root, RESOURCES.check), 'utf8').trim(), "jdownloader=true; var version='rDownloader';")
  assert.equal(readFileSync(join(root, RESOURCES.flash), 'utf8'), 'JDownloader')
  assert.equal(readFileSync(join(root, RESOURCES.done), 'utf8'), 'success')
  const manifest = JSON.parse(readFileSync(join(root, 'manifest.base.json'), 'utf8'))
  const exposed = manifest.web_accessible_resources.flatMap((entry) => entry.resources)
  assert.deepEqual(exposed.slice().sort(), Object.values(RESOURCES).sort(), 'exactly the three answers are web-accessible')
})

test('a refusal is told with its own words, a success with the count', () => {
  assert.deepEqual(handoverOutcome({ ok: true, links: 2 }), { key: 'cnlSent', substitutions: ['2'] })
  assert.deepEqual(handoverOutcome({ ok: true, code: 'site_rules.pick_waiting', entries: 3 }), { key: 'pickWaiting', substitutions: ['3'] })
  assert.deepEqual(handoverOutcome({ ok: false, status: 400, code: 'cnl.jk_refused' }), { key: 'cnlFailed', reasonKey: 'cnlJkRefused' })
  assert.deepEqual(handoverOutcome({ ok: false, status: 401, code: 'auth.unauthorized' }), { key: 'cnlFailed', reasonKey: 'errorUnauthorized' })
  assert.deepEqual(handoverOutcome({ ok: false, status: 502, code: 'http', message: 'HTTP 502' }), { key: 'cnlFailed', reason: 'HTTP 502' })
})

test('the hand-over goes to the service\'s route under the capture token', async () => {
  const seen = []
  const fetchImpl = async (url, init) => {
    seen.push({ url, init })
    return { ok: true, status: 201, json: async () => ({ candidates: [{}, {}] }) }
  }
  const result = await submitClickAndLoad(CONFIG, { urls: 'https://a.test/x' }, fetchImpl)
  assert.deepEqual(result, { ok: true, status: 201, code: null, message: null, links: 2 })
  assert.equal(seen[0].url, 'http://127.0.0.1:8710/api/v1/capture/click-and-load')
  assert.equal(seen[0].init.headers.authorization, 'Bearer capture-token')
  assert.deepEqual(JSON.parse(seen[0].init.body), { urls: 'https://a.test/x' })
})

/** A browser with one allowed site, the extension's session rules, and the service. */
function setup({ agent = false, config = CONFIG, sites = ['hoster.test'], granted = true, blocking = true, answer } = {}) {
  const calls = []
  let rules = []
  let present = agent
  const api = {
    runtime: { getURL: (path) => `moz-extension://self/${path}` },
    storage: { local: { get: async (defaults) => ({ ...defaults, fileSites: sites }) } },
    permissions: { contains: async () => granted },
    alarms: { create: async (name, options) => calls.push(['alarm', name, options.periodInMinutes]) },
    declarativeNetRequest: {
      updateSessionRules: async ({ removeRuleIds, addRules }) => {
        calls.push(['rules', addRules.length])
        rules = rules.filter((rule) => !removeRuleIds.includes(rule.id)).concat(addRules)
      },
      getSessionRules: async () => rules
    }
  }
  const sent = []
  const notices = []
  const clickAndLoad = createClickAndLoad({
    api,
    loadConfig: async () => config,
    message: (key, substitutions) => (substitutions ? `${key}:${substitutions.join(',')}` : key),
    notify: async (body) => notices.push(body),
    blocking,
    probe: async () => {
      calls.push(['probe'])
      return present
    },
    fetchImpl: async (url, init) => {
      sent.push({ url, body: JSON.parse(init.body) })
      return answer ?? { ok: true, status: 201, json: async () => ({ candidates: [{}, {}] }) }
    }
  })
  return { clickAndLoad, calls, sent, notices, rules: () => rules, startAgent: () => { present = true } }
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 5))
const request = (path, extra = {}) => ({ url: `http://127.0.0.1:9666${path}`, method: 'GET', originUrl: PAGE, ...extra })
const crypted = request('/flash/addcrypted2', {
  method: 'POST',
  requestBody: { formData: { crypted: ['abc'], jk: [JK], passwords: ['pw'], package: ['Release'] } }
})

test('Firefox: without the agent the page sees its button and the links reach the service', async () => {
  const { clickAndLoad, sent, notices } = setup()
  assert.deepEqual(await clickAndLoad.onBeforeRequest(request('/jdcheck.js')), { redirectUrl: `moz-extension://self/${RESOURCES.check}` })
  assert.deepEqual(await clickAndLoad.onBeforeRequest(request('/flash')), { redirectUrl: `moz-extension://self/${RESOURCES.flash}` })
  assert.deepEqual(await clickAndLoad.onBeforeRequest(crypted), { redirectUrl: `moz-extension://self/${RESOURCES.done}` })
  await settle()
  assert.deepEqual(sent, [{
    url: 'http://127.0.0.1:8710/api/v1/capture/click-and-load',
    body: { crypted: 'abc', jk: JK, passwords: 'pw', package: 'Release', source: PAGE }
  }])
  assert.deepEqual(notices, ['cnlSent:2'])
})

test('with the agent running the agent takes over and the extension does nothing', async () => {
  // Firefox: every request goes through to the port, and nothing is handed over.
  const firefox = setup({ agent: true })
  for (const details of [request('/jdcheck.js'), request('/flash'), crypted]) {
    assert.deepEqual(await firefox.clickAndLoad.onBeforeRequest(details), {}, details.url)
  }
  await settle()
  assert.deepEqual(firefox.sent, [])
  assert.deepEqual(firefox.notices, [])
  // Chrome: no rule answers the page, and an add the agent received is not handed over twice.
  const chrome = setup({ agent: true, blocking: false })
  await chrome.clickAndLoad.schedule()
  assert.deepEqual(chrome.rules(), [])
  await chrome.clickAndLoad.onBeforeRequest({ ...crypted, originUrl: undefined, initiator: 'https://hoster.test' })
  await settle()
  assert.deepEqual(chrome.sent, [])
})

test('Chrome: rules for the allowed sites while no agent answers, and the add is handed over', async () => {
  const { clickAndLoad, calls, sent, rules } = setup({ blocking: false })
  await clickAndLoad.schedule()
  assert.deepEqual(calls.filter(([name]) => name === 'alarm'), [['alarm', PROBE_ALARM, 1]])
  assert.deepEqual(rules().map((rule) => rule.condition.initiatorDomains), [['hoster.test'], ['hoster.test'], ['hoster.test']])
  await clickAndLoad.onBeforeRequest({ ...crypted, originUrl: undefined, initiator: 'https://hoster.test' })
  await settle()
  assert.equal(sent.length, 1)
  assert.equal(sent[0].body.jk, JK)
})

test('Chrome: an agent that starts later takes the rules away on the next probe', async () => {
  const { clickAndLoad, rules, startAgent, sent } = setup({ blocking: false })
  await clickAndLoad.schedule()
  assert.equal(rules().length, 3)
  startAgent()
  await clickAndLoad.onAlarm({ name: 'rdownloader-captcha-poll' })
  assert.equal(rules().length, 3, 'another alarm is not this one')
  await clickAndLoad.onAlarm({ name: PROBE_ALARM })
  assert.deepEqual(rules(), [])
  // The agent received the next add; the extension hands nothing over a second time.
  await clickAndLoad.onBeforeRequest({ ...crypted, originUrl: undefined, initiator: 'https://hoster.test' })
  await settle()
  assert.deepEqual(sent, [])
})

test('a site not allowed, the switch off or no token: the request is left alone', async () => {
  for (const options of [
    { sites: [] },
    { granted: false },
    { config: { ...CONFIG, clickAndLoad: false } },
    { config: { ...CONFIG, token: '' } }
  ]) {
    const { clickAndLoad, sent, calls } = setup(options)
    assert.deepEqual(await clickAndLoad.onBeforeRequest(request('/jdcheck.js')), {}, JSON.stringify(options))
    assert.deepEqual(await clickAndLoad.onBeforeRequest(crypted), {}, JSON.stringify(options))
    await settle()
    assert.deepEqual(sent, [])
    assert.ok(!calls.some(([name]) => name === 'probe'), 'the port is not even asked')
  }
  // A preflight cannot be answered from here; it is the agent's or nobody's.
  const { clickAndLoad } = setup()
  assert.deepEqual(await clickAndLoad.onBeforeRequest(request('/flash/add', { method: 'OPTIONS' })), {})
})

test('a jk the service refuses is reported to the person, never to the page', async () => {
  const { clickAndLoad, notices } = setup({
    answer: { ok: false, status: 400, json: async () => ({ code: 'cnl.jk_refused', error: 'refused' }) }
  })
  assert.deepEqual(await clickAndLoad.onBeforeRequest(crypted), { redirectUrl: `moz-extension://self/${RESOURCES.done}` })
  await settle()
  assert.deepEqual(notices, ['cnlFailed:cnlJkRefused'])
})
