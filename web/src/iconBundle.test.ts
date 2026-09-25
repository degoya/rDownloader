// @vitest-environment node
/**
 * Every icon the interface names is drawn from the bundle, and nothing can fetch one (RD-120-48).
 *
 * The screenshot run of 2026-09-23 found `api.iconify.design` in the built bundle and icons
 * missing offline: only Nuxt UI's own icons were bundled, the app's were fetched at run time.
 * This mirrors what the build does — the same scanner, the same options — and adds what the
 * build does not check: a scanned name that no installed collection holds is silently dropped
 * there, which is exactly an icon that would have gone to the network.
 *
 * **What it does not see.** A name composed at run time (`i-lucide-${kind}`) is refused below,
 * but a name that arrives from the server or a plugin manifest is invisible to any scan.
 */
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { IconUsageScanner, generateClientBundleCode, resolveBundleIcons } from '@nuxt/icon/utils'
import { describe, expect, it } from 'vitest'

import viteConfig, { iconClientBundle } from '../vite.config'
import * as offline from './iconifyOffline'

const webRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const sourceRoot = join(webRoot, 'src')

function sources(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) {
      return sources(path)
    }
    return /\.(vue|ts)$/.test(entry.name) && !entry.name.endsWith('.test.ts') ? [path] : []
  })
}

/** Every `i-<collection>-<name>` literal, independent of the build's own scanner. */
function namedIcons(): Map<string, string> {
  const icons = new Map<string, string>()
  for (const file of sources(sourceRoot)) {
    for (const match of readFileSync(file, 'utf8').matchAll(/\bi-(lucide)-([a-z0-9]+(?:-[a-z0-9]+)*)\b/g)) {
      icons.set(`${match[1]}:${match[2]}`, file)
    }
  }
  return icons
}

function installedCollection(prefix: string): Set<string> {
  const path = join(webRoot, 'node_modules/@iconify-json', prefix, 'icons.json')
  const data = JSON.parse(readFileSync(path, 'utf8')) as { icons: object, aliases?: object }
  return new Set([...Object.keys(data.icons), ...Object.keys(data.aliases ?? {})])
}

describe('icon bundle', () => {
  it('names icons only from the one installed collection', () => {
    const foreign: string[] = []
    for (const file of sources(sourceRoot)) {
      const text = readFileSync(file, 'utf8')
      // `i-<prefix>-` inside a class list or an icon prop; Tailwind has no `i-` utilities.
      for (const match of text.matchAll(/["'`\s]i-([a-z0-9]+)-/g)) {
        if (match[1] !== 'lucide') {
          foreign.push(`${match[1]} in ${file}`)
        }
      }
    }
    expect(foreign, 'install its @iconify-json collection and extend this test first').toEqual([])
  })

  it('composes no icon name at run time, which no scan could bundle', () => {
    const composed = sources(sourceRoot).filter(file => /i-[a-z0-9]+-[a-z0-9-]*\$\{/.test(readFileSync(file, 'utf8')))
    expect(composed).toEqual([])
  })

  it('finds every named icon in the installed lucide collection', () => {
    const lucide = installedCollection('lucide')
    const missing = [...namedIcons()].filter(([icon]) => !lucide.has(icon.slice('lucide:'.length)))
    expect(missing.map(([icon, file]) => `${icon} (${file})`)).toEqual([])
  })

  it('bundles every named icon with the options the build uses', async () => {
    const scanner = new IconUsageScanner(iconClientBundle.scan)
    const scanned = await scanner.scanFiles(webRoot)
    const unscanned = [...namedIcons().keys()].filter(icon => !scanned.has(icon))
    expect(unscanned, 'the build scan misses these, so they would not be bundled').toEqual([])

    const { collections, failed } = await resolveBundleIcons({
      icons: [],
      scannedIcons: scanned,
      resolvePaths: [webRoot]
    })
    expect(failed).toEqual([])
    const bundled = new Set(collections.flatMap(collection => Object.keys(collection.icons).map(name => `${collection.prefix}:${name}`)))
    expect([...namedIcons().keys()].filter(icon => !bundled.has(icon))).toEqual([])
    // Throws past the limit, exactly as the build would.
    expect(() => generateClientBundleCode(collections, { sizeLimitKb: iconClientBundle.sizeLimitKb })).not.toThrow()
  })

  it('renders through the offline build, which has no API to fall back to', () => {
    const aliases = viteConfig.resolve?.alias
    const entries = Array.isArray(aliases) ? aliases : []
    const iconify = entries.find(entry => entry.find instanceof RegExp && entry.find.test('@iconify/vue'))
    expect(iconify?.replacement).toBe(join(sourceRoot, 'iconifyOffline.ts'))
    expect(iconify?.find instanceof RegExp && iconify.find.test('@iconify/vue/offline')).toBe(false)

    const offlineBuild = readFileSync(join(webRoot, 'node_modules/@iconify/vue/dist/offline.mjs'), 'utf8')
    expect(offlineBuild).not.toMatch(/iconify\.design|fetch\(/)
  })

  it('offers every name Nuxt UI imports from @iconify/vue', () => {
    const runtime = join(webRoot, 'node_modules/@nuxt/ui/dist/runtime/vue')
    const importers = [join(runtime, 'components/Icon.vue'), join(runtime, 'plugins/icons.js')]
    const imported = importers.flatMap((file) => {
      const statement = readFileSync(file, 'utf8').match(/import\s*\{([^}]*)\}\s*from\s*["']@iconify\/vue["']/)
      return (statement?.[1] ?? '').split(',').map(name => name.trim().split(/\s+as\s+/)[0] ?? '').filter(Boolean)
    })
    expect(imported.length).toBeGreaterThan(0)
    expect(imported.filter(name => !(name in offline))).toEqual([])
  })

  it('registers a bundled icon under the name UIcon asks for', () => {
    const data = { body: '<path d="M0 0h1"/>', width: 24, height: 24 }
    expect(offline.addIcon('lucide:bundle-probe', data)).toBe(true)
    expect(offline.iconLoaded('lucide-bundle-probe')).toBe(true)
    expect(offline.iconLoaded('lucide:bundle-probe')).toBe(true)
    expect(offline.iconLoaded('lucide-not-registered')).toBe(false)
  })
})
