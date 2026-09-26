import { ref, type Ref } from 'vue'

import { api, responseError } from '@/api/client'
import { EMPTY_PREFERENCE, type MirrorPreference } from '@/utils/mirrorGroups'

/** What the mirror actions write back into the collector store. */
interface MirrorActionContext {
  error: Ref<string | null>
  refresh: () => Promise<void>
}

/**
 * The collector store's mirror groups: the standing preference, a mirror chosen by hand and a
 * proposed group taken apart (RD-140-27). They share the store's `error` and refresh it, so the
 * store stays one surface.
 */
export function useMirrorActions({ error, refresh }: MirrorActionContext) {
  /**
   * The standing mirror preference (RD-110-19).
   *
   * Server state, not view state: it decides which member of every group the queue will
   * fetch, so it has to outlive this tab and reach the next package that arrives. The store
   * holds the last answer the server gave, never a value the interface hopes it stored.
   */
  const mirrorPreference = ref<MirrorPreference>({ ...EMPTY_PREFERENCE })

  async function loadMirrorPreference(): Promise<void> {
    const response = await api.GET('/api/v1/collector/mirror-preference')
    if (!response.data) return
    mirrorPreference.value = {
      quality: response.data.quality ?? null,
      language: response.data.language ?? null,
      hoster: response.data.hoster ?? null,
      hidden_hosters: response.data.hidden_hosters ?? []
    }
  }

  async function setMirrorPreference(preference: MirrorPreference): Promise<boolean> {
    const response = await api.PUT('/api/v1/collector/mirror-preference', {
      body: {
        quality: preference.quality ?? null,
        language: preference.language ?? null,
        hoster: preference.hoster ?? null,
        hidden_hosters: preference.hidden_hosters
      }
    })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    mirrorPreference.value = {
      quality: response.data.quality ?? null,
      language: response.data.language ?? null,
      hoster: response.data.hoster ?? null,
      hidden_hosters: response.data.hidden_hosters ?? []
    }
    // The server re-chose every group under the new preference, so the rows on screen are the
    // previous answer until this lands.
    await refresh()
    return true
  }

  /** Chooses one mirror of a group by hand, or hands the group back to the preference. */
  async function chooseMirror(id: string, chosen: boolean): Promise<boolean> {
    const response = chosen
      ? await api.POST('/api/v1/collector/candidates/{id}/mirror', { params: { path: { id } } })
      : await api.DELETE('/api/v1/collector/candidates/{id}/mirror', { params: { path: { id } } })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    await refresh()
    return true
  }

  /**
   * Takes a proposed mirror group apart, so its links stand on their own again (RD-110-34).
   *
   * Only a proposal can be taken apart, and the server is the one that says so: a declared or
   * a name-and-size group is refused there with `collector.mirror_group_not_proposed`, so a
   * row that offers the action wrongly still changes nothing.
   */
  async function dissolveMirror(id: string): Promise<boolean> {
    const response = await api.POST('/api/v1/collector/candidates/{id}/mirror/dissolve', { params: { path: { id } } })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    await refresh()
    return true
  }

  return {
    mirrorPreference,
    loadMirrorPreference,
    setMirrorPreference,
    chooseMirror,
    dissolveMirror
  }
}
