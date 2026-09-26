import type { Ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { CandidateAuthProfileMode, LinkCandidate, MediaFormatCriteria, MediaFormatsResponse, MediaOutputPreview, MediaResolution, ReplayPreview } from '@/api/types'

/** What the candidate actions write back into the collector store. */
interface CandidateActionContext {
  candidates: Ref<LinkCandidate[]>
  error: Ref<string | null>
  refresh: () => Promise<void>
}

/**
 * The collector store's settings on one link: its name, its media format, its cookie profile
 * and the consent for a captured request (RD-140-27). They share the store's `error` and write
 * the answered row back into its `candidates`, so the store stays one surface.
 */
export function useCandidateActions({ candidates, error, refresh }: CandidateActionContext) {
  function replaceCandidate(id: string, candidate: LinkCandidate): void {
    candidates.value = candidates.value.map(item => item.id === id ? candidate : item)
  }

  async function patchCandidate(id: string, body: { file_name?: string, media_variant?: string }): Promise<boolean> {
    const response = await api.PATCH('/api/v1/collector/candidates/{id}', { params: { path: { id } }, body })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    replaceCandidate(id, response.data)
    return true
  }

  async function renameCandidate(id: string, fileName: string): Promise<boolean> {
    return patchCandidate(id, { file_name: fileName })
  }

  async function setMediaVariant(id: string, variantId: string): Promise<boolean> {
    return patchCandidate(id, { media_variant: variantId })
  }

  /** Full format inventory of one media candidate, fetched only when the selector opens. */
  async function fetchMediaFormats(id: string): Promise<MediaFormatsResponse | null> {
    const response = await api.GET('/api/v1/collector/candidates/{id}/media', { params: { path: { id } } })
    if (!response.data) {
      error.value = responseError(response)
      return null
    }
    return response.data
  }

  /**
   * What a set of criteria would resolve to, without storing it.
   *
   * A refusal is returned rather than raised: "nothing matches" is the answer the selector
   * needs to render its explanation, not an error to swallow. Its stable code travels along,
   * because "no formats" and "no audio track" call for different advice than a filter
   * combination that keeps nothing (RD-120-50).
   */
  async function previewMediaSelection(
    id: string,
    criteria: MediaFormatCriteria
  ): Promise<{ resolution: MediaResolution | null, code: string | null }> {
    const response = await api.POST('/api/v1/collector/candidates/{id}/media/preview', {
      params: { path: { id } },
      body: { criteria }
    })
    if (response.data) return { resolution: response.data, code: null }
    const failure = response.error as { code?: string | null } | undefined
    return { resolution: null, code: failure?.code ?? null }
  }

  /**
   * What an output template expands to for one link.
   *
   * Previewed on the server through the same evaluator the download uses, so the path shown
   * is the path that gets written. An invalid template returns its reason as the error.
   */
  async function previewMediaOutput(id: string, template: string): Promise<MediaOutputPreview | { error: string }> {
    const response = await api.POST('/api/v1/collector/candidates/{id}/media/output-preview', {
      params: { path: { id } },
      body: { template }
    })
    return response.data ?? { error: responseError(response) }
  }

  /**
   * Chooses the cookie profile a link is queued with (RD-080-04).
   *
   * `auto` lets the scope decide, `none` deliberately sends nothing, `pinned` names one.
   * The server refuses a profile that does not cover the link, so the error is worth
   * surfacing rather than swallowing.
   */
  async function setAuthProfile(
    id: string,
    mode: CandidateAuthProfileMode,
    profileId?: string
  ): Promise<boolean> {
    const response = await api.PUT('/api/v1/collector/candidates/{id}/auth-profile', {
      params: { path: { id } },
      body: { mode, profile_id: profileId ?? null }
    })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    replaceCandidate(id, response.data)
    return true
  }

  async function setMediaSelection(id: string, criteria: MediaFormatCriteria): Promise<boolean> {
    const response = await api.PUT('/api/v1/collector/candidates/{id}/media/selection', {
      params: { path: { id } },
      body: { criteria }
    })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    replaceCandidate(id, response.data)
    return true
  }

  /** What a captured request would send, for the consent dialog. */
  async function replayPreview(id: string): Promise<ReplayPreview | null> {
    const response = await api.GET('/api/v1/collector/candidates/{id}/replay-preview', {
      params: { path: { id } }
    })
    if (!response.data) {
      error.value = responseError(response)
      return null
    }
    return response.data
  }

  /**
   * Approves one captured request.
   *
   * The hash binds the approval to the template the user actually saw; the server refuses
   * it if the capture changed in between.
   */
  async function grantReplayConsent(
    id: string,
    templateHash: string,
    approvedOrigins: string[]
  ): Promise<boolean> {
    const response = await api.POST('/api/v1/collector/candidates/{id}/replay-consent', {
      params: { path: { id } },
      body: { template_hash: templateHash, approved_origins: approvedOrigins }
    })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    await refresh()
    return true
  }

  /**
   * Withdraws an approval that was granted but never enqueued.
   *
   * The enqueue skips the dialog for a candidate that already carries a consent, so an
   * approval left behind by a cancelled or failed enqueue would send the credentials on the
   * next attempt without asking again. This is the way back out.
   */
  async function revokeReplayConsent(id: string): Promise<boolean> {
    const response = await api.DELETE('/api/v1/collector/candidates/{id}/replay-consent', {
      params: { path: { id } }
    })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    await refresh()
    return true
  }

  return {
    renameCandidate,
    setMediaVariant,
    fetchMediaFormats,
    previewMediaSelection,
    previewMediaOutput,
    setAuthProfile,
    setMediaSelection,
    replayPreview,
    grantReplayConsent,
    revokeReplayConsent
  }
}
