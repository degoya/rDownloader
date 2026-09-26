import { computed, ref, type Ref } from 'vue'

import { api, responseError, resultMessage } from '@/api/client'
import type { PluginRevocation } from '@/api/types'

/**
 * One build the operator is about to withdraw, as the dialog names it.
 *
 * The identity the service stores is the package digest, but nobody recognises a plugin by 64
 * hex characters — so the request goes out as the id and the version the card already shows,
 * and the service resolves which exact package that was.
 */
export interface PendingWithdrawal {
  id: string
  version: string
  name: string
}

/**
 * The plugin tab's withdrawn packages: the list, whether a card's build is on it, and the
 * dialog that adds one (split out of `SettingsPluginsTab.vue`, RD-140-27). Outcomes land in
 * the tab's own `message` and `error`.
 */
export function usePluginWithdrawals(tab: {
  message: Ref<string | null>
  error: Ref<string | null>
}) {
  const { message, error } = tab
  /** Withdrawn packages, newest first as the service lists them. */
  const revocations = ref<PluginRevocation[]>([])
  const pendingWithdrawal = ref<PendingWithdrawal | null>(null)
  const withdrawalReason = ref('')
  const withdrawing = ref(false)

  /**
   * The withdrawals that name an installed version, as `<id>@<version>`.
   *
   * A withdrawal is stored by digest, and a digest says nothing to a reader — so the card that
   * carries the name and the version is where it has to be visible. An entry whose package is
   * not installed here matches nothing and is named in the list below instead, where its digest
   * is the only honest answer.
   */
  const withdrawnVersions = computed(() => new Set(
    revocations.value
      .filter(entry => entry.plugin_id && entry.version)
      .map(entry => `${entry.plugin_id}@${entry.version}`)
  ))

  function isWithdrawn(plugin: { id: string, version: string }): boolean {
    return withdrawnVersions.value.has(`${plugin.id}@${plugin.version}`)
  }

  async function refreshRevocations(): Promise<string | null> {
    const response = await api.GET('/api/v1/plugins/revocations')
    if (!response.data) return responseError(response)
    revocations.value = response.data
    return null
  }

  /** Opens the dialog for one exact build; the reason starts empty for every one of them. */
  function askWithdraw(name: string, id: string, version: string): void {
    withdrawalReason.value = ''
    pendingWithdrawal.value = { id, version, name }
  }

  /**
   * Withdraws the build the dialog names.
   *
   * The list is fetched again rather than patched: the service answers with the digest it
   * resolved, the context columns and the moment, and guessing any of those here would be a
   * second source for what the service already states.
   */
  async function withdraw(): Promise<void> {
    const target = pendingWithdrawal.value
    if (!target) return
    pendingWithdrawal.value = null
    error.value = null
    message.value = null
    withdrawing.value = true
    const reason = withdrawalReason.value.trim()
    const response = await api.POST('/api/v1/plugins/revocations', {
      body: { plugin_id: target.id, version: target.version, ...(reason ? { reason } : {}) }
    })
    if (response.data) message.value = resultMessage(response.data)
    else error.value = responseError(response)
    const failure = await refreshRevocations()
    if (failure) error.value = failure
    withdrawing.value = false
  }

  /** Takes a withdrawal back. Reversible in both directions, which is why neither asks twice. */
  async function liftWithdrawal(digest: string): Promise<void> {
    error.value = null
    message.value = null
    const response = await api.DELETE('/api/v1/plugins/revocations/{digest}', {
      params: { path: { digest } }
    })
    if (response.data) message.value = resultMessage(response.data)
    else error.value = responseError(response)
    const failure = await refreshRevocations()
    if (failure) error.value = failure
  }

  return { revocations, pendingWithdrawal, withdrawalReason, withdrawing, isWithdrawn, refreshRevocations, askWithdraw, withdraw, liftWithdrawal }
}
