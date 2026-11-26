// @vitest-environment node
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { gunzipSync } from 'node:zlib'

import { afterEach, describe, expect, it } from 'vitest'

import { precompress, precompressPlugin } from './precompress'

describe('the precompressed copies of the build', () => {
  let directory = ''
  afterEach(() => rmSync(directory, { recursive: true, force: true }))

  it('follow an output directory given as an absolute path', () => {
    directory = mkdtempSync(join(tmpdir(), 'rd-precompress-'))
    writeFileSync(join(directory, 'app.js'), 'const answer = 42;\n'.repeat(200))
    const plugin = precompressPlugin() as unknown as {
      configResolved: (config: unknown) => void
      closeBundle: { handler: () => void }
    }
    plugin.configResolved({ root: '/somewhere/else', build: { outDir: directory } })
    plugin.closeBundle.handler()
    expect(readdirSync(directory).sort()).toEqual(['app.js', 'app.js.gz'])
  })

  it('lie beside every text file they make smaller, and beside nothing else', () => {
    directory = mkdtempSync(join(tmpdir(), 'rd-precompress-'))
    mkdirSync(join(directory, 'assets'))
    const script = 'const answer = 42;\n'.repeat(200)
    writeFileSync(join(directory, 'assets', 'app.js'), script)
    writeFileSync(join(directory, 'index.html'), '<!doctype html>'.repeat(200))
    writeFileSync(join(directory, 'assets', 'tiny.js'), 'export {}')
    writeFileSync(join(directory, 'assets', 'font.woff2'), script)

    expect(precompress(directory)).toBe(2)
    expect(gunzipSync(readFileSync(join(directory, 'assets', 'app.js.gz'))).toString()).toBe(script)
    expect(readdirSync(directory, { recursive: true, encoding: 'utf8' }).filter(name => name.endsWith('.gz')).sort())
      .toEqual([join('assets', 'app.js.gz'), 'index.html.gz'])
    // The copies of the last build are no text file of this one.
    expect(precompress(directory)).toBe(2)
  })
})
