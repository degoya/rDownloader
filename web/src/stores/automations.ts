import { defineStore } from 'pinia'
import { ref } from 'vue'

import { api, responseError } from '@/api/client'
import { clearWhenReconnected } from '@/composables/serviceConnection'
import { debouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import { useLatestFetch } from '@/composables/useLatestFetch'
import type {
  Automation,
  AutomationDryRun,
  AutomationRequest,
  AutomationRun,
  AutomationVersion,
  AutomationVocabulary
} from '@/api/types'

/**
 * Automations, their run history and the vocabulary the editor builds its forms from.
 *
 * The vocabulary comes from the server rather than being written out here twice: a trigger
 * or operator the editor offers but the API refuses would be a form that cannot be saved,
 * and that is exactly the kind of drift a second hand-kept list produces.
 */
export const useAutomationsStore = defineStore('automations', () => {
  const automations = ref<Automation[]>([])
  const runs = ref<AutomationRun[]>([])
  const vocabulary = ref<AutomationVocabulary | null>(null)
  const error = ref<string | null>(null)
  // A "service could not be reached" alert ends with the outage.
  clearWhenReconnected(error)
  const busy = ref(false)
  /**
   * The list's fetch (`busy` covers the write actions); `loading` is its first fetch alone, and
   * only the newest refresh lands (WEB-06).
   */
  const { fetching, loading, run } = useLatestFetch()

  async function refresh(): Promise<void> {
    try {
      await run(
        () => Promise.all([api.GET('/api/v1/automations'), api.GET('/api/v1/automations/runs')]),
        ([list, history]) => {
          if (list.data) {
            automations.value = list.data
            error.value = null
          } else {
            error.value = responseError(list)
          }
          if (history.data) runs.value = history.data
        }
      )
    } catch {
      error.value = responseError(undefined)
    }
  }

  /**
   * Loads the trigger/condition/action catalogue that drives every dropdown in the editor.
   *
   * A failure used to be swallowed: `vocabulary` stayed null, each `?? []` fallback produced an
   * empty list, and the view showed four empty dropdowns with no error at all. It now reports
   * the failure like every other action here, and a later call retries instead of being turned
   * away by the "already loaded" guard.
   */
  async function loadVocabulary(): Promise<void> {
    if (vocabulary.value) return
    const response = await api.GET('/api/v1/automations/vocabulary')
    if (response.data) {
      vocabulary.value = response.data
      error.value = null
    } else {
      error.value = responseError(response)
    }
  }

  /** Saves an automation and answers with the stored one, or `null` with `error` set. */
  async function save(body: AutomationRequest, id?: string): Promise<Automation | null> {
    busy.value = true
    const response = id
      ? await api.PUT('/api/v1/automations/{id}', { params: { path: { id } }, body })
      : await api.POST('/api/v1/automations', { body })
    busy.value = false
    if (!response.data) {
      error.value = responseError(response)
      return null
    }
    error.value = null
    await refresh()
    return response.data
  }

  async function setEnabled(id: string, enabled: boolean): Promise<void> {
    const response = await api.POST('/api/v1/automations/{id}/enable', {
      params: { path: { id } },
      body: { enabled }
    })
    if (!response.data) {
      error.value = responseError(response)
      return
    }
    await refresh()
  }

  async function remove(id: string): Promise<void> {
    const response = await api.DELETE('/api/v1/automations/{id}', { params: { path: { id } } })
    if (!response.data) {
      error.value = responseError(response)
      return
    }
    await refresh()
  }

  /**
   * The saved versions of one automation, newest first (RD-190-22), or `null` with `error` set.
   * Read on demand rather than kept: only the history dialog asks, for one automation at a time.
   */
  async function versions(id: string): Promise<AutomationVersion[] | null> {
    const response = await api.GET('/api/v1/automations/{id}/versions', { params: { path: { id } } })
    if (!response.data) {
      error.value = responseError(response)
      return null
    }
    error.value = null
    return [...response.data].sort((left, right) => right.version - left.version)
  }

  /**
   * Puts an older version's trigger, condition and actions back in force (RD-190-22).
   *
   * There is no restore route and none is needed: the ordinary update stores a new version, so
   * the restored definition is the newest one and the versions in between stay in the history.
   * Name and switch are the automation's current ones — a version holds neither.
   */
  async function restore(automation: Automation, version: AutomationVersion): Promise<Automation | null> {
    return save({
      name: automation.name,
      enabled: automation.enabled,
      trigger: version.trigger,
      condition: version.condition,
      actions: version.actions
    }, automation.id)
  }

  /** Evaluates a trigger and a sample package. Never has an effect. */
  async function dryRun(trigger: string, packageId: string | null): Promise<AutomationDryRun[]> {
    const response = await api.POST('/api/v1/automations/dry-run', {
      body: { trigger, package_id: packageId ?? undefined } as never
    })
    if (!response.data) {
      error.value = responseError(response)
      return []
    }
    error.value = null
    return response.data
  }

  /**
   * What the bus says about automations.
   *
   * This store subscribed to nothing until now, because the server published nothing: an
   * automation created, enabled, disabled or deleted in a second tab — or by an import — was
   * invisible here until the page was reloaded, and the toggle a user then flipped was
   * flipped against a list from minutes ago.
   *
   * Re-read rather than patched. The payload names the automation that changed, but this store
   * holds two lists from two endpoints: `refresh()` reads the automations *and* the run
   * history, and a run the change produced cannot be reconstructed from an id. Every write
   * here already ends in the same `refresh()`, so the event path is the one the store is
   * built around rather than a second way of maintaining the same state. Debounced because a
   * bulk import emits one event per automation. No notice is raised — `design.md` has no
   * pattern for announcing that data caught up.
   */
  const { connect: connectEvents, disconnect: disconnectEvents } =
    debouncedEventRefresh(['automation.changed'], refresh, { busy: () => fetching.value })

  return {
    automations,
    runs,
    vocabulary,
    error,
    busy,
    loading,
    refresh,
    loadVocabulary,
    save,
    setEnabled,
    remove,
    dryRun,
    versions,
    restore,
    connectEvents,
    disconnectEvents
  }
})
