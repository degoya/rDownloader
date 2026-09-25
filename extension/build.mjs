// Builds dist/chrome and dist/firefox from one code base (no dependencies).
import { cpSync, mkdirSync, readFileSync, rmSync, writeFileSync, existsSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { execFileSync } from 'node:child_process'

const root = dirname(fileURLToPath(import.meta.url))
const defaultOutDir = join(root, '..', 'artifacts', 'browser-extensions')

export const TARGETS = ['chrome', 'firefox']

/**
 * The oldest browsers this project supports, as the project owner stated them on 2026-09-20:
 * Firefox 156 and Chrome 153, and nothing below. They are declared rather than left open, so a
 * browser that is too old refuses the install instead of running untested (RD-109-47).
 */
export const FIREFOX_MIN_VERSION = '156.0'
export const CHROME_MIN_VERSION = '153'

/** Permissions only the Firefox build carries: what copying a response takes (RD-130-16). */
export const FIREFOX_ONLY_PERMISSIONS = ['webRequestBlocking', 'webRequestFilterResponse']

export function archiveName(target) {
  return `rdownloader-${target}.zip`
}

export function manifestFor(target, base) {
  const manifest = structuredClone(base)
  if (target === 'chrome') {
    manifest.background = { service_worker: 'src/background.js', type: 'module' }
    manifest.minimum_chrome_version = CHROME_MIN_VERSION
  } else if (target === 'firefox') {
    manifest.background = { scripts: ['src/background.js'], type: 'module' }
    // The floor is what the project actually supports, not the oldest Firefox that would
    // probably work. 115 was an ESR guess nobody tested; the project develops against the
    // current release and says so, so a browser below it refuses the install instead of
    // running untested (RD-109-47). Raise it when the supported version moves.
    manifest.browser_specific_settings = { gecko: { id: 'rdownloader@degoya.de', strict_min_version: FIREFOX_MIN_VERSION } }
    // The response copy of RD-130-16: `filterResponseData` needs both, and a Manifest V3 add-on
    // the second one as well. Neither reaches a page without the host grant the person gives per
    // site. Chrome has no such API and refuses `webRequestBlocking` outside a policy install.
    manifest.permissions = [...manifest.permissions, ...FIREFOX_ONLY_PERMISSIONS]
  } else {
    throw new Error(`unknown target ${target}`)
  }
  return manifest
}

// The closing line of a build names what is actually on disk. It used to test the Chrome archive
// and speak for both, so a run that wrote one of the two read as if it had written both.
export function archiveSummary(written) {
  const present = TARGETS.filter((target) => written.includes(target))
  const missing = TARGETS.filter((target) => !written.includes(target))
  if (missing.length === 0) return `both archives written (${present.map(archiveName).join(', ')})`
  if (present.length === 0) return `no archive written (missing ${missing.map(archiveName).join(', ')})`
  return `only ${present.map(archiveName).join(', ')} written (missing ${missing.map(archiveName).join(', ')})`
}

/**
 * Writes both unpacked targets and, unless told otherwise, their archives.
 *
 * `archive: false` exists for the test that runs this function: `scripts/build-extension.sh
 * --test-only` promises to need nothing but Node, and calling `zip` from a unit test would
 * quietly break that promise (RD-109-26). A release always archives.
 */
export function build(outDir = defaultOutDir, { archive: writeArchives = true } = {}) {
  const base = JSON.parse(readFileSync(join(root, 'manifest.base.json'), 'utf8'))
  rmSync(outDir, { recursive: true, force: true })
  for (const target of TARGETS) {
    const dir = join(outDir, target)
    mkdirSync(dir, { recursive: true })
    for (const entry of ['src', 'icons', '_locales']) cpSync(join(root, entry), join(dir, entry), { recursive: true })
    writeFileSync(join(dir, 'manifest.json'), JSON.stringify(manifestFor(target, base), null, 2))
    if (!writeArchives) continue
    const archive = join(outDir, archiveName(target))
    // The archive is what a release publishes and what a store accepts — never the unpacked
    // directory — so a `zip` that is missing or fails is a build failure, not a detail to swallow.
    try {
      execFileSync('zip', ['-qr', archive, '.'], { cwd: dir, stdio: ['ignore', 'ignore', 'pipe'] })
    } catch (error) {
      const detail = error.stderr?.toString().trim() || error.message
      throw new Error(`could not write ${archive}: ${detail}`, { cause: error })
    }
  }
  return outDir
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const out = build()
  // `zip` can report success without leaving the file where it was asked to; the message states
  // what is there rather than what was attempted.
  const written = TARGETS.filter((target) => existsSync(join(out, archiveName(target))))
  console.log(`built ${out}/chrome and ${out}/firefox — ${archiveSummary(written)}`)
  if (written.length !== TARGETS.length) process.exitCode = 1
}
