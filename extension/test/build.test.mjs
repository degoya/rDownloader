import assert from 'node:assert/strict'
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

import { CHROME_MIN_VERSION, FIREFOX_MIN_VERSION, FIREFOX_ONLY_PERMISSIONS, TARGETS, archiveSummary, build, manifestFor } from '../build.mjs'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const base = JSON.parse(readFileSync(join(root, 'manifest.base.json'), 'utf8'))
const ICONS = ['icons/icon16.png', 'icons/icon32.png', 'icons/icon48.png', 'icons/icon128.png']

test('chrome uses a service worker, firefox background scripts with a gecko id', () => {
  const chrome = manifestFor('chrome', base)
  assert.equal(chrome.background.service_worker, 'src/background.js')
  assert.equal(chrome.browser_specific_settings, undefined)
  const firefox = manifestFor('firefox', base)
  assert.deepEqual(firefox.background.scripts, ['src/background.js'])
  assert.equal(firefox.browser_specific_settings.gecko.id, 'rdownloader@degoya.de')
  assert.equal(base.background, undefined, 'base manifest must stay browser-neutral')
})

test('only the Firefox build may block a request and copy a response', () => {
  // RD-130-16: `filterResponseData` takes both permissions, and it does nothing without the host
  // grant the person gives per site. Chrome refuses `webRequestBlocking` outside a policy install,
  // so the base manifest and the Chrome build must never carry either.
  assert.deepEqual(FIREFOX_ONLY_PERMISSIONS, ['webRequestBlocking', 'webRequestFilterResponse'])
  const firefox = manifestFor('firefox', base)
  const chrome = manifestFor('chrome', base)
  for (const permission of FIREFOX_ONLY_PERMISSIONS) {
    assert.ok(firefox.permissions.includes(permission), `Firefox lacks ${permission}`)
    assert.ok(!chrome.permissions.includes(permission), `${permission} reached Chrome`)
    assert.ok(!base.permissions.includes(permission), `${permission} is in the base manifest`)
  }
  assert.deepEqual(base.host_permissions, chrome.host_permissions, 'no host is granted up front for it')
  assert.deepEqual(firefox.host_permissions, base.host_permissions)
})

test('the base manifest requests the download interception permissions', () => {
  for (const permission of ['contextMenus', 'storage', 'notifications', 'clipboardRead', 'downloads']) {
    assert.ok(base.permissions.includes(permission), `missing permission ${permission}`)
  }
  // `webRequest` stays after RD-109-20 removed the POST-body path, and for one reason only:
  // observation. It is what carries the method of a request, the allowlisted request headers
  // and the Content-Disposition into the handoff — without it `correlateRequest` can never
  // return anything, a form-triggered download cannot be recognised as one, and the handoff
  // would replay it as a GET. `extension/test/replay.test.mjs` holds the other half: no
  // listener asks for `requestBody`.
  assert.ok(base.permissions.includes('webRequest'), 'observation: method, headers, Content-Disposition')
  // Widget captchas (RD-108-02): the poll runs on an alarm and the reader is injected with
  // `scripting`; neither shows an install-time warning. The hoster's origin itself is optional
  // and requested per captcha, so `<all_urls>` must never become a mandatory host permission.
  for (const permission of ['alarms', 'scripting']) {
    assert.ok(base.permissions.includes(permission), `missing permission ${permission}`)
  }
  assert.ok(!base.host_permissions.some((pattern) => /\*:\/\/\*|<all_urls>|https?:\/\/\*\/\*/.test(pattern)), 'hoster origins must stay optional')
  // Each wildcard names the feature that would stop working without it. RD-109-20's first
  // finding read them as the gate of the removed POST-body path and asked for their deletion;
  // `git show 678fd64f -- extension/manifest.base.json` shows the array unchanged as context in
  // the very commit that added the interception, so they predate that path and are not its.
  // A per-origin `permissions.request({ origins })` for an origin no manifest pattern covers is
  // refused by Chrome and by Firefox, so deleting these disables three live features at once.
  for (const [wildcard, needed] of [
    ['https://*/*', 'captcha.js requests one hoster origin per widget, session.js one site origin'],
    ['http://*/*', 'options.js requests the origin of an rDownloader server that is not loopback']
  ]) {
    assert.ok(base.optional_host_permissions.includes(wildcard), `${wildcard} is what makes it legal: ${needed}`)
  }
  // Cookies are requested per origin at share time, so installing the extension must not
  // warn about cookie access to every site.
  assert.ok(!base.permissions.includes('cookies'), 'cookies must never be a mandatory permission')
  assert.ok(base.optional_permissions.includes('cookies'), 'cookies must be optional')
  // Firefox validates this array and refuses an entry it does not know, which cost a release:
  // `requestBody` is an extraInfoSpec flag on a webRequest listener, never a permission
  // (RD-108-22). Everything here must be a permission both browsers actually define.
  const KNOWN_OPTIONAL = ['cookies', 'tabs', 'webNavigation', 'history', 'bookmarks']
  for (const permission of base.optional_permissions) {
    assert.ok(KNOWN_OPTIONAL.includes(permission), `${permission} is not a browser permission`)
  }
  // Reading the active tab's address is what the popup's "send this page" needs; without it
  // tabs.query answers without a url and the button reports that the page has no address.
  assert.ok(base.permissions.includes('activeTab'), 'the popup needs the active tab\'s address')
})

test('the manifest declares the fields without which the extension does not load', () => {
  assert.equal(base.manifest_version, 3)
  assert.equal(base.default_locale, 'en')
  assert.match(base.version, /^\d+(\.\d+){0,3}$/)
  assert.equal(base.action.default_popup, 'src/popup.html')
  assert.equal(base.action.default_title, '__MSG_extName__')
  assert.deepEqual(base.options_ui, { page: 'src/options.html', open_in_tab: false })
  assert.deepEqual(Object.keys(base.icons).sort((a, b) => Number(a) - Number(b)), ['16', '32', '48', '128'])
  // Every optional host permission, not just the one that used to be spot-checked: an entry
  // nobody looked at is an entry that can be added or dropped without anybody noticing.
  assert.deepEqual(base.optional_host_permissions.slice().sort(), ['http://*/*', 'https://*/*'])
})

test('a real build writes two manifests, both pages and all four icons', () => {
  // `build()` itself was never called by a test: only `manifestFor` was, so nothing checked
  // that the files the manifest points at end up in the output (RD-109-26).
  const out = mkdtempSync(join(tmpdir(), 'rdownloader-extension-'))
  try {
    // No archives: `--test-only` promises to need nothing but Node, and `zip` is not Node.
    build(out, { archive: false })
    for (const target of TARGETS) {
      const dir = join(out, target)
      const manifest = JSON.parse(readFileSync(join(dir, 'manifest.json'), 'utf8'))
      assert.equal(manifest.manifest_version, 3, target)
      assert.equal(manifest.version, base.version, target)
      for (const path of [manifest.action.default_popup, manifest.options_ui.page, ...ICONS]) {
        assert.ok(existsSync(join(dir, path)), `${target} is missing ${path}`)
      }
      assert.ok(existsSync(join(dir, '_locales', 'en', 'messages.json')), `${target} ships no catalogue`)
    }
    const firefox = JSON.parse(readFileSync(join(out, 'firefox', 'manifest.json'), 'utf8'))
    assert.equal(firefox.background.service_worker, undefined, 'a Chrome-only key reached Firefox')
    assert.deepEqual(firefox.background.scripts, ['src/background.js'])
    // The floor is a statement about support, so it is asserted against the constant rather
    // than a literal: the two must not be able to drift (RD-109-47).
    assert.equal(firefox.browser_specific_settings.gecko.strict_min_version, FIREFOX_MIN_VERSION)
    assert.match(FIREFOX_MIN_VERSION, /^\d+\.\d+$/, 'a Firefox floor is a version, not a range')
    const chromeManifest = JSON.parse(readFileSync(join(out, 'chrome', 'manifest.json'), 'utf8'))
    assert.equal(chromeManifest.minimum_chrome_version, CHROME_MIN_VERSION)
    assert.equal(chromeManifest.browser_specific_settings, undefined, 'a Firefox-only key reached Chrome')
    assert.equal(firefox.minimum_chrome_version, undefined, 'a Chrome-only key reached Firefox')
    const chrome = JSON.parse(readFileSync(join(out, 'chrome', 'manifest.json'), 'utf8'))
    assert.equal(chrome.browser_specific_settings, undefined)
    assert.equal(chrome.background.scripts, undefined)
  } finally {
    rmSync(out, { recursive: true, force: true })
  }
})

test('every message key still has a reader', () => {
  // The catalogue test below compares the four languages with each other and never with the
  // code, so `permissionRequestBody` and its description sat there in all four for weeks after
  // the permission they belonged to was removed (RD-109-25). A key is referenced as
  // `message('key')` in a source file, as `data-i18n="key"` in one of the two pages, or as
  // `__MSG_key__` in the manifest.
  const english = JSON.parse(readFileSync(join(root, '_locales', 'en', 'messages.json'), 'utf8'))
  const readers = [join(root, 'manifest.base.json')]
  for (const name of readdirSync(join(root, 'src'))) {
    if (/\.(js|html)$/.test(name)) readers.push(join(root, 'src', name))
  }
  const text = readers.map((path) => readFileSync(path, 'utf8')).join('\n')
  const orphans = Object.keys(english).filter(
    (key) => !new RegExp(`(['"\`]${key}['"\`]|__MSG_${key}__)`).test(text)
  )
  assert.deepEqual(orphans, [], 'these keys are translated four times and read nowhere')
})

/** Which `$1`, `$2`, … a message entry refers to, in its text or in its placeholder table. */
function placeholderSlots(entry) {
  return [...new Set(JSON.stringify(entry).match(/\$\d/g) ?? [])].sort()
}

test('every locale ships the same message keys as English, filled and with the same slots', () => {
  const dir = join(root, '_locales')
  const english = JSON.parse(readFileSync(join(dir, 'en/messages.json'), 'utf8'))
  const keys = Object.keys(english).sort()
  // `readdirSync` alone threw on any stray file in this directory — a `.DS_Store`, an editor
  // backup — and a test that falls over on a file nobody put there is a test people learn to
  // ignore (RD-109-26).
  const locales = readdirSync(dir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
  assert.deepEqual(locales.slice().sort(), ['de', 'en', 'es', 'fr'])

  for (const locale of locales) {
    const catalogue = JSON.parse(readFileSync(join(dir, locale, 'messages.json'), 'utf8'))
    assert.deepEqual(Object.keys(catalogue).sort(), keys, `locale ${locale}`)
    for (const [key, entry] of Object.entries(catalogue)) {
      // An empty message renders as an empty label, which reads as a bug in the UI rather than
      // as a missing translation.
      assert.ok(String(entry.message ?? '').trim(), `${locale}/${key} has no text`)
      assert.deepEqual(
        placeholderSlots(entry),
        placeholderSlots(english[key]),
        `${locale}/${key} uses different substitutions than English`
      )
    }
  }

  for (const key of ['extName', 'menuLink', 'menuPage', 'menuSelection', 'menuSession', 'interceptSending', 'interceptKeep', 'sessionShared', 'captchaWaiting', 'captchaAnswer']) {
    assert.ok(keys.includes(key))
  }
})

test('a stray file in the locales directory is ignored, not fatal', () => {
  const stray = join(root, '_locales', '.rdownloader-test-stray')
  writeFileSync(stray, 'not a catalogue')
  try {
    const locales = readdirSync(join(root, '_locales'), { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name)
    assert.deepEqual(locales.slice().sort(), ['de', 'en', 'es', 'fr'])
  } finally {
    rmSync(stray, { force: true })
  }
})

// The extension is the one delivered artifact that used to sit out the version bump: the manifest
// said 0.1.0 while the workspace said 1.0.8, so every release published the same version number
// and no store could have accepted an update (RD-109-17). `scripts/set-version.sh` writes the
// field now, and this test is what makes a forgotten bump red instead of silently wrong.
test('the manifest carries the workspace version', () => {
  const cargo = readFileSync(join(root, '..', 'Cargo.toml'), 'utf8')
  const section = cargo.split('[workspace.package]')[1]
  assert.ok(section, 'Cargo.toml has no [workspace.package] section')
  const workspace = /\nversion = "([^"]+)"/.exec(section.split('\n[')[0])
  assert.ok(workspace, 'could not read the workspace version from Cargo.toml')
  // A pre-release suffix is a Cargo concept; a browser manifest takes dot-separated integers
  // only, so the manifest carries the release core of the workspace version.
  const expected = workspace[1].replace(/[-+].*$/, '')
  assert.equal(base.version, expected, `manifest.base.json says ${base.version}, the workspace says ${workspace[1]}`)
  assert.match(base.version, /^\d+(\.\d+){0,3}$/, 'a browser manifest version is one to four integers')
})

test('the closing message tells both archives, one and none apart', () => {
  assert.match(archiveSummary(['chrome', 'firefox']), /^both archives written/)
  assert.match(archiveSummary(['chrome']), /^only rdownloader-chrome\.zip written \(missing rdownloader-firefox\.zip\)/)
  assert.match(archiveSummary(['firefox']), /^only rdownloader-firefox\.zip written \(missing rdownloader-chrome\.zip\)/)
  assert.match(archiveSummary([]), /^no archive written \(missing rdownloader-chrome\.zip, rdownloader-firefox\.zip\)/)
})
