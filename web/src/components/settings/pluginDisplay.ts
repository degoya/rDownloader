import type { InstalledPlugin } from '@/api/types'
import { providerText } from '@/i18n/plugins'

export interface TrustedKey {
  key_id: string
  fingerprint: string
  plugin_name: string | null
  confirmed_at: string
}

/** Localised plugin name, falling back to the manifest's own value. */
export function displayName(plugin: InstalledPlugin): string {
  return providerText(plugin.provider_slug, 'name') ?? plugin.name
}

export function pluginDescription(plugin: InstalledPlugin): string {
  return providerText(plugin.provider_slug, 'description') ?? plugin.description
}

/** Splits a hex fingerprint into 8-character blocks so it can be compared by eye. */
export function groupFingerprint(fingerprint: string): string {
  return (fingerprint.match(/.{1,8}/g) ?? [fingerprint]).join(' ')
}
