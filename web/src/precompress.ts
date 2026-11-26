import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { gzipSync } from 'node:zlib'

import type { Plugin } from 'vite'

/** Text only: fonts and pictures are compressed as they are. */
const TEXT = /\.(?:css|html|js|json|svg|webmanifest)$/
/** Below this a response fits one packet either way. */
const MIN_BYTES = 1_024

/**
 * Writes `name.gz` beside every text file of `directory` that gets smaller by it, and answers how
 * many. Gzip alone: browsers ask for Brotli over HTTPS only, and the service is mostly opened over
 * plain HTTP in the local network.
 */
export function precompress(directory: string): number {
  let written = 0
  for (const entry of readdirSync(directory, { recursive: true, encoding: 'utf8' })) {
    const path = join(directory, entry)
    if (!TEXT.test(entry) || !statSync(path).isFile()) continue
    const plain = readFileSync(path)
    if (plain.length < MIN_BYTES) continue
    const packed = gzipSync(plain, { level: 9 })
    if (packed.length >= plain.length) continue
    writeFileSync(`${path}.gz`, packed)
    written += 1
  }
  return written
}

/**
 * Compresses the build's files once, when it is built (RD-1480-19): they are embedded in the
 * binary and served as they are (`crates/rd-api/src/static_assets.rs`), so the service never
 * spends CPU on them and needs no compression layer that could reach an answer with secrets.
 */
export function precompressPlugin(): Plugin {
  let directory = ''
  return {
    name: 'rdownloader:precompress',
    apply: 'build',
    configResolved(config) {
      // `--outDir` may be absolute (scripts/web-declarations.sh): `join` would put it under the root.
      directory = resolve(config.root, config.build.outDir)
    },
    closeBundle: { order: 'post', sequential: true, handler: () => { precompress(directory) } }
  }
}
