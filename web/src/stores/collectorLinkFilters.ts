import type { Ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { LinkFilterApplyResponse } from '@/api/types'

/** What the LinkFilter actions write back into the collector store. */
interface LinkFilterActionContext {
  error: Ref<string | null>
  refresh: () => Promise<void>
}

/**
 * The collector store's half of the LinkFilter rules (RD-1240-09): applying them to the list as
 * it is, and showing a link one of them hid. Both change rows on the server, so both refresh.
 */
export function useLinkFilterActions({ error, refresh }: LinkFilterActionContext) {
  /** Decides every link anew by the rules; answers what changed, or `null` with `error` set. */
  async function applyLinkFilters(): Promise<LinkFilterApplyResponse | null> {
    const response = await api.POST('/api/v1/link-filters/apply')
    if (!response.data) {
      error.value = responseError(response)
      return null
    }
    error.value = null
    await refresh()
    return response.data
  }

  /** Shows hidden links until the rules are applied again. */
  async function unhideCandidates(ids: string[]): Promise<boolean> {
    if (!ids.length) return true
    const response = await api.POST('/api/v1/collector/candidates/unhide', { body: { candidate_ids: ids } })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    await refresh()
    return true
  }

  return { applyLinkFilters, unhideCandidates }
}
