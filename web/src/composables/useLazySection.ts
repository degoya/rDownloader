import { defineAsyncComponent, type Component } from 'vue'
import { useRouter } from 'vue-router'

import { isChunkLoadError, reportViewLoadFailure } from '@/composables/serviceConnection'

/**
 * Part of a view as a chunk of its own, fetched when it is first shown. One that cannot be
 * fetched is reported the way a whole view is (`router.ts`), with a reload once the service is back.
 */
export function useLazySection(): <T extends Component>(load: () => Promise<{ default: T }>) => T {
  const router = useRouter()
  return load => defineAsyncComponent(() => load().then(module => module.default, (error: unknown) => {
    if (isChunkLoadError(error)) reportViewLoadFailure(router.currentRoute.value.fullPath)
    throw error
  }))
}
