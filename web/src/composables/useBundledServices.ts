import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { installBundled, listBundled, type BundledInstallFailure, type BundledService } from '@/api/bundledPlugins'
import { useFetchState } from '@/composables/useFetchState'
import { translateServerMessage } from '@/i18n/server'

/** What one install run did, for the caller to say. */
export interface BundledInstallOutcome {
  installed: number
  failures: BundledInstallFailure[]
  /** A refused request (not one plugin failing), in the reader's language. */
  error: string | null
}

/**
 * The bundle by service, and installing from it (RD-160-05), shared by the wizard's "Your
 * services" step and the plugin manager's "Available" list.
 *
 * A run installs one service per request, so the progress it reports is real: the bar moves when
 * a service has landed, not on a timer. One service failing does not stop the others.
 */
export function useBundledServices() {
  const { locale } = useI18n()
  const services = ref<BundledService[]>([])
  const fetchState = useFetchState()
  /** `{ done, total }` while a run is installing, `null` otherwise. */
  const progress = ref<{ done: number, total: number } | null>(null)
  const installing = computed(() => progress.value !== null)

  async function refresh(): Promise<string | null> {
    const answer = await listBundled(locale.value)
    if (!answer.ok) return translateServerMessage(answer.message)
    services.value = Array.isArray(answer.data?.services) ? answer.data.services : []
    return null
  }

  async function install(keys: string[]): Promise<BundledInstallOutcome> {
    const outcome: BundledInstallOutcome = { installed: 0, failures: [], error: null }
    if (!keys.length) return outcome
    let done = 0
    progress.value = { done, total: keys.length }
    for (const key of keys) {
      const answer = await installBundled([key])
      if (answer.ok) {
        outcome.installed += answer.data.installed?.length ?? 0
        outcome.failures.push(...(answer.data.failed ?? []))
      } else {
        outcome.error = translateServerMessage(answer.message)
      }
      done += 1
      progress.value = { done, total: keys.length }
    }
    progress.value = null
    await refresh()
    return outcome
  }

  return {
    services,
    loading: fetchState.loading,
    loadError: fetchState.loadError,
    load: () => fetchState.load(refresh),
    refresh,
    progress,
    installing,
    install
  }
}
