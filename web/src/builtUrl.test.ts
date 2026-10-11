// @vitest-environment node
/**
 * Every address the build or the source writes holds under a mount point (RD-1250-09).
 *
 * Behind a proxy that passes only `/rd/` to the service, Vite's preloads, the fonts of the CSS and
 * the hard-coded `/favicon.svg` were asked for at the root and answered 404: four settings pages
 * stayed empty and the logo was missing. `builtUrl.ts` decides how the build writes them; this
 * holds that decision and keeps the source from writing an absolute one by hand again.
 */
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

import { renderBuiltUrl } from './builtUrl'

const sourceRoot = dirname(fileURLToPath(import.meta.url))

type HostType = 'js' | 'css' | 'html'

function render(hostType: HostType) {
  return renderBuiltUrl('assets/chunk-abc.js', { type: 'asset', hostId: 'host', hostType, ssr: false })
}

/** Evaluates the run-time expression the build writes into a chunk, as a page would. */
function evaluate(runtime: string, base: string | undefined): string {
  const window = base === undefined ? {} : { __RD_BASE__: base }
  return (new Function('window', `return ${runtime}`) as (w: object) => string)(window)
}

describe('built addresses', () => {
  it('builds a chunk address in JavaScript from the base the server injected', () => {
    const result = render('js')
    expect(result).toMatchObject({ runtime: expect.any(String) })
    const { runtime } = result as { runtime: string }
    expect(evaluate(runtime, '/rd')).toBe('/rd/assets/chunk-abc.js')
    expect(evaluate(runtime, '')).toBe('/assets/chunk-abc.js')
    expect(evaluate(runtime, undefined)).toBe('/assets/chunk-abc.js')
  })

  it('refers to fonts and pictures relative to the stylesheet', () => {
    expect(render('css')).toEqual({ relative: true })
  })

  it('leaves the HTML its absolute paths, which the server prefixes', () => {
    expect(render('html')).toBeUndefined()
  })
})

/** The one place the mark's address is written, through the base. */
const LOGO_HOME = 'basePath.ts'

function sources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap(entry => {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) return sources(path)
    return /\.(vue|ts|css)$/.test(entry.name) && !/\.(test|spec)\.ts$/.test(entry.name) ? [path] : []
  })
}

describe('hand-written addresses', () => {
  it('never names a public file or a built asset by its absolute path', () => {
    const absolute = /["'`(]\/(?:favicon\.svg|assets\/)/
    const offenders = sources(resolve(sourceRoot)).flatMap(file => {
      const name = relative(sourceRoot, file)
      if (name === LOGO_HOME) return []
      return readFileSync(file, 'utf8')
        .split('\n')
        .flatMap((line, index) => (absolute.test(line) ? [`${name}:${index + 1}: ${line.trim()}`] : []))
    })
    expect(offenders, 'use LOGO_URL or withBase from basePath.ts').toEqual([])
  })

  it('writes the mark through the base', () => {
    expect(readFileSync(join(sourceRoot, LOGO_HOME), 'utf8')).toContain("withBase('/favicon.svg')")
  })
})
