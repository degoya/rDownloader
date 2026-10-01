import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { Indexer } from '@/api/types'

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

  /** Fetches the list; answers the translated failure, or `null`. */
  async function refresh(): Promise<string | null> {
    const response = await api.GET('/api/v1/indexers')
    if (!response.data) return responseError(response)
    indexers.value = [...response.data].sort((left, right) => left.name.localeCompare(right.name))
    loaded.value = true
    return null
  }

  return { indexers, loaded, enabled, refresh }
})
