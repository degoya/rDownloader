import { ref, type Ref } from 'vue'

import { responseError } from '@/api/client'
import { useLatestFetch, type LatestFetch } from '@/composables/useLatestFetch'

/** A page of a newest-first record list that pages backwards by `before_id`. */
interface RecordsPage<Entry> {
  records: Entry[]
}

/**
 * The newest-first record list the audit log and the structured log share (WEB-07): a first
 * page under the current filter, and older pages appended behind the oldest record shown.
 *
 * Both stores had their own copy, and in both `refresh()` let a `loadOlder()` still on its way
 * finish: the page of the old filter was appended under the new one. The ticket of
 * `useLatestFetch` drops it now — a refresh is the newer fetch.
 *
 * `read` asks for one page, the newest without `beforeId`; `take` keeps what a page says besides
 * its records (totals, retention); `error` is the store's own, which its other actions share.
 */
export function usePagedRecords<Entry extends { id: number }, Page extends RecordsPage<Entry>>(
  read: (beforeId?: number) => Promise<{ data?: Page }>,
  take: (page: Page) => void,
  error: Ref<string | null>
): Pick<LatestFetch, 'fetching' | 'settled' | 'loading'> & {
  records: Ref<Entry[]>
  refresh: () => Promise<void>
  loadOlder: () => Promise<void>
} {
  const records = ref([]) as Ref<Entry[]>
  const { fetching, settled, loading, run } = useLatestFetch()

  function apply(response: { data?: Page }, append: boolean): void {
    if (!response.data) {
      error.value = responseError(response)
      return
    }
    records.value = append ? [...records.value, ...response.data.records] : response.data.records
    take(response.data)
    error.value = null
  }

  async function refresh(): Promise<void> {
    await run(() => read(), response => apply(response, false))
  }

  /** Appends the page behind the oldest record shown. */
  async function loadOlder(): Promise<void> {
    const oldest = records.value.at(-1)
    if (!oldest || fetching.value) return
    await run(() => read(oldest.id), response => apply(response, true))
  }

  return { records, fetching, settled, loading, refresh, loadOlder }
}
