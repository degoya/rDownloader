import { ref } from 'vue'

import { api, responseError, resultMessage } from '@/api/client'
import type { InstalledPlugin, PluginLifecycle } from '@/api/types'

/** A resolver version under test that a single download can be started with (RD-140-02). */
interface StagedResolver {
  pluginId: string
  name: string
  version: string
  domains: string[]
}

/**
 * The staged resolver versions a download can be tried on right now.
 *
 * Only a version the running service has loaded counts: staging takes effect at a restart, and
 * the trial route refuses a version that is only stored, so offering it before would be a menu
 * entry that answers "restart first". A choice waiting for a restart is therefore left out.
 */
export function stagedResolvers(installed: InstalledPlugin[], lifecycle: PluginLifecycle[]): StagedResolver[] {
  const staged: StagedResolver[] = []
  for (const entry of lifecycle) {
    if (!entry.staged_version || entry.restart_required) continue
    const plugin = installed.find(candidate =>
      String(candidate.id) === entry.plugin_id && candidate.version === entry.staged_version)
    if (!plugin || plugin.plugin_type !== 'resolver') continue
    staged.push({ pluginId: entry.plugin_id, name: plugin.name, version: plugin.version, domains: plugin.domains })
  }
  return staged
}

/** The host rule the resolver itself applies (`rd_plugin_host::domain_allowed`). */
export function domainAllowed(source: string, domains: string[]): boolean {
  let url: URL
  try {
    url = new URL(source)
  } catch {
    return false
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return false
  const host = url.hostname.toLowerCase()
  return domains.some(domain => domain === '*'
    || (domain.startsWith('*.') && host.endsWith(`.${domain.slice(2)}`))
    || host === domain)
}

/** Shared across every queue row: one inventory read, not one per row. */
const staged = ref<StagedResolver[]>([])
let loaded = false

async function load(): Promise<void> {
  const response = await api.GET('/api/v1/plugins')
  const inventory = response.data
  staged.value = inventory && Array.isArray(inventory.installed) && Array.isArray(inventory.lifecycle)
    ? stagedResolvers(inventory.installed, inventory.lifecycle)
    : []
}

export function useStagedResolvers() {
  if (!loaded) {
    loaded = true
    void load()
  }

  /** The staged resolver that would handle `source`, if one is under test. */
  function stagedFor(source: string): StagedResolver | null {
    return staged.value.find(resolver => domainAllowed(source, resolver.domains)) ?? null
  }

  /** Points the download at the staged version for its next start. */
  async function trial(resolver: StagedResolver, downloadId: string): Promise<{ message: string | null, error: string | null }> {
    const response = await api.POST('/api/v1/plugins/{id}/lifecycle/trial', {
      params: { path: { id: resolver.pluginId } },
      body: { download_id: downloadId }
    })
    return response.data
      ? { message: resultMessage(response.data), error: null }
      : { message: null, error: responseError(response) }
  }

  return { staged, stagedFor, trial, refresh: load }
}
