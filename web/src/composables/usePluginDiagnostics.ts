import { ref, type Ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { InstalledPlugin, PluginExecution } from '@/api/types'

/**
 * The plugins tab's diagnostics accordion: one plugin's recorded invocations open at a time,
 * fetched the first time they are asked for (split out of `SettingsPluginsTab.vue`, RD-140-27).
 * A failed fetch lands in the tab's own `error`.
 */
export function usePluginDiagnostics(error: Ref<string | null>) {
  /** Loaded on demand per plugin: diagnostics nobody opened cost nothing. */
  const executions = ref<Record<string, PluginExecution[]>>({})
  const openDiagnostics = ref<string | null>(null)
  /**
   * The plugin whose entries are in flight, so the open panel says "loading" rather than looking
   * like a plugin that recorded nothing. It is never both: a fetch that fails clears this and
   * raises `error`, which the card already shows in an alert of its own.
   */
  const diagnosticsLoading = ref<string | null>(null)

  /** Shows or hides one plugin's recorded invocations, fetching them the first time. */
  async function toggleDiagnostics(plugin: InstalledPlugin): Promise<void> {
    if (openDiagnostics.value === plugin.id) {
      openDiagnostics.value = null
      return
    }
    openDiagnostics.value = plugin.id
    diagnosticsLoading.value = plugin.id
    const response = await api.GET('/api/v1/plugins/{id}/executions', { params: { path: { id: plugin.id } } })
    if (response.data) executions.value = { ...executions.value, [plugin.id]: response.data }
    else error.value = responseError(response)
    // Only if nothing else moved on in the meantime: closing the panel, or opening another
    // plugin's, takes the flag from that moment, and a late answer must not clear it for them.
    if (diagnosticsLoading.value === plugin.id) diagnosticsLoading.value = null
  }

  return { executions, openDiagnostics, diagnosticsLoading, toggleDiagnostics }
}
