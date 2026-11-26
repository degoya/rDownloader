// A throwaway extension that asks whether Firefox still holds its click rule (RD-1480-04).
//
// The Firefox run answers the permission prompt through a preference
// (`extensions.webextOptionalPermissionPrompts=false`), and that preference must not take the
// other half with it: `permissions.request` is valid only while Firefox handles a person's input
// (bug 1398833), the rule RD-1480-01 broke three times. The prompt is the parent process's, the
// rule is checked in the process of the caller before the prompt is reached — but that is an
// implementation detail, so the run asks this Firefox, with this profile, instead of relying on
// it. The probe requests in four ways; only the two made inside a handler may be granted.
import { mkdirSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

export const PROBE_ID = 'gesture-probe@e2e.invalid'

/** What Firefox says to a request outside a handler of a person's input. */
export const REFUSED = /may only be called from a user input handler/

/** The probe's outcomes, under the ids of the elements `probe.html` shows them in. */
export const OUTCOMES = ['load', 'button-direct', 'button-late', 'menu-direct', 'menu-late']

const MANIFEST = {
  manifest_version: 3,
  name: 'Gesture probe',
  version: '1',
  permissions: ['storage', 'contextMenus'],
  optional_host_permissions: ['*://*/*'],
  background: { scripts: ['probe.js', 'background.js'] },
  browser_specific_settings: { gecko: { id: PROBE_ID, data_collection_permissions: { required: ['none'] } } }
}

// Shared by the background and the page. A host of its own per outcome: nothing is held already.
const SHARED = `
const ask = (outcome) => browser.permissions.request({ origins: ['*://' + outcome + '.probe.test/*'] })
function attempt(outcome, work) {
  let asked
  try { asked = work() } catch (error) { asked = Promise.reject(error) }
  Promise.resolve(asked)
    .then((granted) => (granted ? 'granted' : 'denied'), (error) => 'refused: ' + error.message)
    .then((result) => browser.storage.local.set({ [outcome]: result }))
}
const direct = (outcome) => attempt(outcome, () => ask(outcome))
// The mistake of RD-1480-01: something is awaited before the request.
const late = (outcome) => attempt(outcome, () => browser.storage.local.get('nothing').then(() => ask(outcome)))
`

const BACKGROUND = `
browser.runtime.onInstalled.addListener(() => {
  for (const id of ['menu-direct', 'menu-late']) browser.contextMenus.create({ id, title: id, contexts: ['page'] })
})
browser.contextMenus.onClicked.addListener((info) => {
  if (info.menuItemId === 'menu-direct') direct('menu-direct')
  if (info.menuItemId === 'menu-late') late('menu-late')
})
`

const PAGE = `<!doctype html><meta charset="utf-8"><title>Gesture probe</title>
<button id="ask-direct">direct</button> <button id="ask-late">late</button>
${OUTCOMES.map((id) => `<p id="${id}"></p>`).join('\n')}
<script src="probe.js"></script><script src="page.js"></script>`

const PAGE_SCRIPT = `
document.getElementById('ask-direct').addEventListener('click', () => direct('button-direct'))
document.getElementById('ask-late').addEventListener('click', () => late('button-late'))
const show = () => browser.storage.local.get().then((all) => {
  for (const [id, result] of Object.entries(all)) document.getElementById(id).textContent = result
})
browser.storage.onChanged.addListener(show)
show()
direct('load')
`

/** Writes the probe below `workDir` and returns its directory. */
export function writeProbe(workDir) {
  const directory = join(workDir, 'gesture-probe')
  mkdirSync(directory, { recursive: true })
  const files = { 'manifest.json': JSON.stringify(MANIFEST, null, 2), 'probe.js': SHARED, 'background.js': BACKGROUND, 'probe.html': PAGE, 'page.js': PAGE_SCRIPT }
  for (const [name, content] of Object.entries(files)) writeFileSync(join(directory, name), content)
  return directory
}
