import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { Indexer, IndexerCaps } from '@/api/types'

/**
 * The Newznab indexers defined once under Settings › Usenet (RD-180-19).
 *
 * One list for the three places that read it: the settings card that edits it, the
 * LinkGrabber's search, which exists only while at least one indexer is enabled, and the
 * subscription form, which can take an indexer over. The API key never reaches it; a row says
 * only whether one is stored.
 */
export const useIndexersStore = defineStore('indexers', () => {
  const indexers = ref<Indexer[]>([])
  /** True once a fetch has answered; until then nobody knows whether there are indexers. */
  const loaded = ref(false)

  const enabled = computed(() => indexers.value.filter(indexer => indexer.enabled))

  /**
   * Each indexer's `t=caps` answer, asked only when the search first needs it and then kept until
   * the list is fetched again (RD-1100-03): which search types it answers and which ids each
   * takes. `null` is a test that failed; such an indexer is offered the plain search alone.
   */
  const caps = ref<Record<string, IndexerCaps | null>>({})
  const asking = new Map<string, Promise<void>>()

  /** Asks the indexers whose answer is not known yet; resolves once every one has answered. */
  async function loadCaps(ids: readonly string[]): Promise<void> {
    await Promise.all(ids.filter(id => !(id in caps.value)).map(id => {
      let pending = asking.get(id)
      if (!pending) {
        pending = api.POST('/api/v1/indexers/{id}/caps', { params: { path: { id } } }).then(response => {
          caps.value = { ...caps.value, [id]: response.data ?? null }
        }).finally(() => asking.delete(id))
        asking.set(id, pending)
      }
      return pending
    }))
  }

  /** Fetches the list; answers the translated failure, or `null`. */
  async function refresh(): Promise<string | null> {
    const response = await api.GET('/api/v1/indexers')
    if (!response.data) return responseError(response)
    indexers.value = [...response.data].sort((left, right) => left.name.localeCompare(right.name))
    loaded.value = true
    // An edited indexer may answer differently now: its caps are asked again when next needed.
    caps.value = {}
    return null
  }

  return { indexers, loaded, enabled, caps, loadCaps, refresh }
})
