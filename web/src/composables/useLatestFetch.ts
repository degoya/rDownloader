import { computed, ref, type ComputedRef, type Ref } from 'vue'

/**
 * A store's own fetch: its flags, and a ticket so only the newest answer lands (WEB-06).
 *
 * Seven stores kept `fetching`, `settled` and `loading` beside `useFetchState`, and only two of
 * them — the transfer list and the LinkGrabber — guarded against answers arriving out of order.
 * In the others an older answer that came in last overwrote a newer one: a filter changed while
 * the previous page was still on its way showed the old filter's records under the new one.
 * `useFetchState` stays the component form; a store also has writes that end in a refresh and a
 * flag the event debounce waits for, which is what this adds.
 */
export interface LatestFetch {
  /** True while a fetch is on its way; the event debounce waits for it. */
  fetching: Ref<boolean>
  /** True once the first fetch has settled, one way or the other. */
  settled: Ref<boolean>
  /**
   * What a view shows in place of an empty list: the **first** fetch alone (RD-104-07). Not
   * `fetching || !settled` (RD-106-19): every write ends in a refresh, and an empty list traded
   * its empty state for the loading surface and back on each of them.
   */
  loading: ComputedRef<boolean>
  /**
   * Runs `request` and hands its result to `apply`, unless a newer `run` started in the
   * meantime. Answers whether it was applied. A rejection reaches the caller; the flags
   * come down either way, so a debounce waiting on `fetching` cannot wait for good.
   */
  run: <T>(request: () => Promise<T>, apply: (result: T) => void) => Promise<boolean>
}

export function useLatestFetch(): LatestFetch {
  const fetching = ref(false)
  const settled = ref(false)
  const loading = computed(() => !settled.value)
  let ticket = 0

  async function run<T>(request: () => Promise<T>, apply: (result: T) => void): Promise<boolean> {
    const mine = ++ticket
    fetching.value = true
    try {
      const result = await request()
      if (mine !== ticket) return false
      apply(result)
      return true
    } finally {
      if (mine === ticket) {
        fetching.value = false
        settled.value = true
      }
    }
  }

  return { fetching, settled, loading, run }
}
