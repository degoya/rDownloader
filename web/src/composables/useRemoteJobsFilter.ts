import { ref, watch, type Ref } from 'vue'
import { useRoute, useRouter, type LocationQuery } from 'vue-router'

import type { RemoteJobState } from '@/api/types'
import type { RemoteJobFilter } from '@/components/settings/remoteJobsFilter'

const STATES: readonly RemoteJobState[] = ['submitting', 'preparing', 'awaiting_choice', 'working', 'ready', 'failed', 'discarded']

function first(value: LocationQuery[string] | undefined): string {
  const head = Array.isArray(value) ? value[0] : value
  return typeof head === 'string' ? head.trim().toLowerCase() : ''
}

function fromQuery(query: LocationQuery): RemoteJobFilter {
  const provider = first(query.provider)
  const state = first(query.state)
  return {
    provider: provider || 'all',
    state: (STATES as readonly string[]).includes(state) ? state as RemoteJobState : 'all'
  }
}

/**
 * The remote jobs list's provider and state filter, kept in the address as `?provider=` and
 * `?state=` (RD-1200-01, `design.md`: *A list's filter and search live in the address*).
 *
 * A reload keeps what was being looked at -- and with it what *Clear list* would act on -- and a
 * link opens the list narrowed. The default carries no query, and the address is replaced, not
 * pushed. A provider is taken as written, since the list of providers arrives later; an unknown
 * state is no filter.
 */
export function useRemoteJobsFilter(): Ref<RemoteJobFilter> {
  const route = useRoute()
  const router = useRouter()
  const filter = ref<RemoteJobFilter>(fromQuery(route.query))

  watch(filter, (next) => {
    const query: LocationQuery = { ...route.query }
    delete query.provider
    delete query.state
    if (next.provider !== 'all') query.provider = next.provider
    if (next.state !== 'all') query.state = next.state
    if (first(route.query.provider) === (query.provider ?? '') && first(route.query.state) === (query.state ?? '')) return
    void router.replace({ path: route.path, query, hash: route.hash })
  }, { deep: true })

  // A link followed while the list is open lands here too.
  watch(() => [route.query.provider, route.query.state] as const, () => {
    const wanted = fromQuery(route.query)
    if (wanted.provider !== filter.value.provider || wanted.state !== filter.value.state) filter.value = wanted
  })

  return filter
}
