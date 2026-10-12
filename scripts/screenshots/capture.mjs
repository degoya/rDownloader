// Takes the views of views.mjs from the running demo service (RD-1270-16).
//
//   node capture.mjs --list [--group G] [--views a,b]     print the views selected, take nothing
//   node capture.mjs --group G [--views a,b] [--scales 1,2] [--version X.Y.Z]
//
// Writes <RD_SHOTS_WORK>/out/<scale>x/<view>.png at targets.json's viewport, dark, English.
// --version shows that version where the service names its own (a package built before the
// release's version bump); --scales defaults to the scales targets.json asks for. Playwright comes
// from RD_SHOTS_PLAYWRIGHT (shots.sh: web/node_modules), the browser from RD_SHOTS_CHROMIUM or
// Playwright's own. Every HTTP error and page error is listed under PROBLEMS at the end.
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { GROUPS, selectViews } from './views.mjs'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const MANIFEST = JSON.parse(fs.readFileSync(path.join(HERE, 'targets.json'), 'utf8'))

function option(name) {
  const at = process.argv.indexOf(name)
  return at > 0 ? process.argv[at + 1] : undefined
}

const group = option('--group')
if (group && !GROUPS.includes(group)) {
  console.error(`unknown group '${group}' (${GROUPS.join(', ')})`)
  process.exit(2)
}
let selected
try {
  selected = selectViews(option('--views'), group)
} catch (e) {
  console.error(e.message)
  process.exit(2)
}

if (process.argv.includes('--list')) {
  for (const v of selected) console.log(`${v.group.padEnd(8)} ${v.name}`)
  process.exit(0)
}
if (!group) {
  console.error('--group is required to capture: each group needs its own demo state (shots.sh capture runs them in turn)')
  process.exit(2)
}

const { BASE, MOCK, WORK, password } = await import('./lib.mjs')
const PLAYWRIGHT = process.env.RD_SHOTS_PLAYWRIGHT
const { chromium } = await import(PLAYWRIGHT ? pathToFileURL(PLAYWRIGHT).href : 'playwright')
const capture = MANIFEST.capture
const SCALES = (option('--scales') ?? [...new Set(Object.values(MANIFEST.targets).map(t => t.scale))].join(','))
  .split(',').map(Number)
const problems = []

const health = await (await fetch(`${BASE}/api/v1/health`)).json().catch(() => ({}))
const RUNNING = health?.version
const VERSION = option('--version') ?? RUNNING
console.log(`service ${RUNNING ?? 'unknown'}, shown as ${VERSION ?? 'unknown'}; ${selected.length} view(s), scales ${SCALES.join(',')}`)

async function settle(page, extra = 1200) {
  await page.waitForFunction(() => !/\bLoading\b|Checking tools/.test(document.body.innerText), null, { timeout: 60000 })
    .catch(() => problems.push(`${page.url()}: still loading`))
  await page.waitForTimeout(extra)
  // The mouse off anything hoverable, so no tooltip sits in the shot.
  await page.mouse.move(capture.width - 1, capture.height - 1)
}

async function newContext(browser, scale, session) {
  const context = await browser.newContext({
    viewport: { width: capture.width, height: capture.height }, deviceScaleFactor: scale,
    colorScheme: capture.color_scheme, locale: capture.browser_locale, timezoneId: capture.timezone
  })
  await context.addInitScript(([locale, scheme]) => {
    localStorage.setItem('rd.locale', locale)
    localStorage.setItem('vueuse-color-scheme', scheme)
  }, [capture.locale, capture.color_scheme])
  if (RUNNING && VERSION && RUNNING !== VERSION) {
    await context.route('**/api/v1/health', async (route) => {
      const response = await route.fetch()
      const body = await response.json()
      if (body?.version) body.version = VERSION
      await route.fulfill({ response, json: body })
    })
    await context.route('**/assets/*.js', async (route) => {
      const response = await route.fetch()
      const text = (await response.text()).replaceAll(`"${RUNNING}"`, `"${VERSION}"`).replaceAll(`\`${RUNNING}\``, `\`${VERSION}\``)
      await route.fulfill({ response, body: text })
    })
  }
  // Indexer covers point at example hosts the browser cannot resolve; the mock draws them.
  await context.route(/^http:\/\/covers\.example\.net\//, async (route) => {
    const url = new URL(route.request().url())
    const response = await fetch(`${MOCK}${url.pathname}${url.search}`, { headers: { host: url.host } })
    await route.fulfill({ status: response.status, contentType: response.headers.get('content-type') ?? 'image/svg+xml', body: Buffer.from(await response.arrayBuffer()) })
  })
  if (session) {
    const r = await context.request.post(`${BASE}/api/v1/auth/login`, { data: { password: password() }, headers: { origin: BASE } })
    if (!r.ok()) throw new Error(`login failed: ${r.status()}`)
  }
  return context
}

function watch(page) {
  page.on('response', (r) => { if (r.status() >= 400 && r.url().startsWith(BASE)) problems.push(`HTTP ${r.status()} ${r.url().replace(BASE, '')}`) })
  page.on('pageerror', e => problems.push(`pageerror ${e.message}`))
}

const browser = await chromium.launch(process.env.RD_SHOTS_CHROMIUM ? { executablePath: process.env.RD_SHOTS_CHROMIUM } : {})
try {
  for (const scale of SCALES) {
    const shared = new Map()
    const contextFor = async (v) => {
      const session = v.session !== false
      if (v.fresh) return newContext(browser, scale, session)
      if (!shared.has(session)) shared.set(session, await newContext(browser, scale, session))
      return shared.get(session)
    }
    for (const v of selected) {
      const context = await contextFor(v)
      const page = await context.newPage()
      watch(page)
      await page.goto(BASE + v.route, { waitUntil: 'domcontentloaded' })
      await settle(page)
      if (v.act) {
        try { await v.act(page) } catch (e) { problems.push(`${v.name}: ${e.message.split('\n')[0]}`) }
        await settle(page, 900)
      }
      const dir = path.join(WORK, 'out', `${scale}x`)
      fs.mkdirSync(dir, { recursive: true })
      await page.screenshot({ path: path.join(dir, `${v.name}.png`) })
      console.log(`${scale}x ${v.name}`)
      await page.close()
      if (v.fresh) await context.close()
    }
    for (const context of shared.values()) await context.close()
  }
} finally {
  await browser.close()
}
if (problems.length) {
  console.log('PROBLEMS:\n' + [...new Set(problems)].join('\n'))
  process.exitCode = 1
}
