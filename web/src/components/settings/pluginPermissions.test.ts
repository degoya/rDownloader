/**
 * Every grant the service lists reads as a sentence in every language (RD-180-22).
 *
 * `capabilityLabel` builds its key from the grant's name, so a grant without a catalogue entry
 * shows as the raw key — `plugins.capability.key_derivation` stood on the MEGA card that way.
 * The list mirrors `Capabilities::granted` in `crates/rd-plugin-host/src/manifest.rs`; a grant
 * added there belongs here too.
 */
import { describe, expect, it } from 'vitest'

import de from '@/locales/de/plugins.json'
import en from '@/locales/en/plugins.json'
import es from '@/locales/es/plugins.json'
import fr from '@/locales/fr/plugins.json'

import { capabilityLabel } from './pluginPermissions'

const GRANTED = ['net_http', 'cookies', 'captcha', 'net_stream:21,990', 'secrets:mega_session', 'key_derivation']

/** A `t` over one catalogue that answers a missing key with the key, as vue-i18n does. */
function translator(catalogue: Record<string, unknown>) {
  return (key: string, named: Record<string, unknown>): string => {
    const value = key.replace(/^plugins\./, '').split('.')
      .reduce<unknown>((node, part) => (node as Record<string, unknown> | undefined)?.[part], catalogue)
    if (typeof value !== 'string') return key
    return value.replace(/\{(\w+)\}/g, (_, name: string) => String(named[name]))
  }
}

describe('capabilityLabel', () => {
  it.each([['de', de], ['en', en], ['es', es], ['fr', fr]])('names every grant in %s', (_, catalogue) => {
    const t = translator(catalogue)
    for (const grant of GRANTED) {
      expect(capabilityLabel(t, grant), grant).not.toMatch(/^plugins\./)
    }
    expect(capabilityLabel(t, 'secrets:mega_session')).toContain('mega_session')
    expect(capabilityLabel(t, 'net_stream:21,990')).toContain('21,990')
  })
})
