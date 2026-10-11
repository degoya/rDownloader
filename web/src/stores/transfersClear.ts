import { ref, type Ref } from 'vue'

import { api } from '@/api/client'
import { payloadError, t, type ClearResult, type ClearScope } from './transfersShared'

/** What clearing the list writes back into the transfers store. */
interface ClearContext {
  error: Ref<string | null>
  notice: Ref<string | null>
  refresh: () => Promise<void>
}

/**
 * The transfers store's "clear the list": one server-side decision over whole packages, and what
 * it removed put into words. Shares the store's `error` and `notice` (WEB-13).
 */
export function useClearList({ error, notice, refresh }: ClearContext) {
  const clearing = ref(false)

  /**
   * Clears the list on the server, in one request, over whole packages.
   *
   * It used to pick single rows here and delete them one at a time, in up to fifteen rounds.
   * Nothing in that chain asked what else was in the package, so "remove completed" tore the
   * finished rows out of a package that was still downloading and left the files behind with
   * nothing that knew they belonged together (RD-107-07). The rule is now one server-side
   * decision.
   *
   * Through the client like every request (WEB-02): a dropped connection is a coded refusal, and
   * `clearing` comes down in `finally` — it stood for good after a network error, and every
   * later clear returned at once.
   *
   * `everything` is only sent from its own confirmation, so it carries `confirmed` — the server
   * refuses that scope without it — and the answer to "delete partial files as well".
   */
  async function clear(scope: ClearScope, deletePartial = false): Promise<void> {
    if (clearing.value) return
    clearing.value = true
    notice.value = null
    error.value = null
    const body = scope === 'everything' ? { scope, confirmed: true, delete_partial: deletePartial } : { scope }
    let result: ClearResult | undefined
    try {
      const response = await api.POST('/api/v1/packages/clear', { body })
      if (!response.data) {
        // The refresh has to come first: on success it clears `error`, so a message set before
        // it would be wiped and the refusal would read as a completed clear.
        await refresh()
        error.value = payloadError(response.error)
        return
      }
      result = response.data
    } finally {
      clearing.value = false
    }
    const removed = result.removed
    await refresh()
    // Only what happened (owner, 2026-10-10): the packages it removed, never the ones it left.
    notice.value = removed
      ? t('downloads.notices.cleared_packages', { count: removed }, removed)
      : t('downloads.notices.nothing_to_clear')
  }

  return { clear, clearing }
}
