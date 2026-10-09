import { ref, type Ref } from 'vue'

import { api } from '@/api/client'
import { translateServerMessage } from '@/i18n/server'

import { payloadError, t, type ClearResult, type ClearScope, type ClearSkip } from './transfersShared'

/** What clearing the list writes back into the transfers store. */
interface ClearContext {
  error: Ref<string | null>
  notice: Ref<string | null>
  warning: Ref<string | null>
  refresh: () => Promise<void>
}

/**
 * The transfers store's "clear the list": one server-side decision over whole packages, and what
 * it refused to touch put into words. Shares the store's `error`, `notice` and `warning` (WEB-13).
 */
export function useClearList({ error, notice, warning, refresh }: ClearContext) {
  const clearing = ref(false)

  /**
   * Clears the list on the server, in one request, over whole packages.
   *
   * It used to pick single rows here and delete them one at a time, in up to fifteen rounds.
   * Nothing in that chain asked what else was in the package, so "remove completed" tore the
   * finished rows out of a package that was still downloading and left the files behind with
   * nothing that knew they belonged together (RD-107-07). The rule is now one server-side
   * decision, and what it refused to touch comes back with a reason.
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
    const skipped = result.skipped
    await refresh()
    if (!removed && !skipped.length) {
      notice.value = t('downloads.notices.nothing_to_clear')
      return
    }
    // A package left alone is a warning that stays: its name is what somebody acts on (RD-1220-03).
    const sentence = [
      t('downloads.notices.cleared_packages', { count: removed }, removed),
      ...skipReasons(skipped)
    ].join(' ')
    if (skipped.length) warning.value = sentence
    else notice.value = sentence
  }

  /**
   * One sentence per reason, not one per package: a list of thirty names is unreadable, and the
   * reason is what tells somebody whether to wait or to act.
   */
  function skipReasons(skipped: ClearSkip[]): string[] {
    const byCode = new Map<string, string[]>()
    for (const entry of skipped) {
      const names = byCode.get(entry.code)
      if (names) names.push(entry.name)
      else byCode.set(entry.code, [entry.name])
    }
    return [...byCode].map(([code, names]) => t('downloads.notices.clear_skipped', {
      count: names.length,
      reason: translateServerMessage({ code, message: code }),
      names: names.slice(0, 3).join(', ')
    }, names.length))
  }

  return { clear, clearing }
}
