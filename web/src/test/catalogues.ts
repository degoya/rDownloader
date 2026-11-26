import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { REQUIRED_LOCALES } from '@/i18n/languages'

const locales = resolve(dirname(fileURLToPath(import.meta.url)), '../locales')

function flatten(value: unknown, prefix: string, out: Map<string, string>): void {
  if (typeof value === 'string') out.set(prefix, value)
  else if (typeof value === 'object' && value !== null) {
    for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
      flatten(child, prefix.endsWith(':') ? `${prefix}${key}` : `${prefix}.${key}`, out)
    }
  }
}

/** Every string of one language's bundled catalogues as `<catalogue>:<key>`. */
export function catalogueStrings(locale: string): Map<string, string> {
  const directory = join(locales, locale)
  const out = new Map<string, string>()
  for (const name of readdirSync(directory).filter(file => file.endsWith('.json')).sort()) {
    const catalogue: unknown = JSON.parse(readFileSync(join(directory, name), 'utf8'))
    flatten(catalogue, `${name.replace(/\.json$/, '')}:`, out)
  }
  return out
}

/** One string of one language, or a loud failure: a renamed key must not turn a check green. */
export function catalogueString(strings: Map<string, string>, key: string): string {
  const text = strings.get(key)
  if (text === undefined) throw new Error(`no catalogue string ${key}`)
  return text
}

export { REQUIRED_LOCALES }
