import { getCurrentScope, onScopeDispose, ref, type Ref } from 'vue'

import { debouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import type { ApiResult } from '@/composables/useEditableList'

/** One copy of a server read, as the stores of `sharedRead` hold it. */
interface SharedRead<T, R> {
  /** The last answer; the initial value until one arrived. A failed read leaves it as it was. */
  value: Ref<T>
  /**
   * Reads from the server and answers the response itself, so a caller keeps reading `data` and
   * its error as it did from `api.GET`. A read already on its way is joined, not repeated.
   */
  load: () => Promise<R>
  /** From a component's setup: keeps `value` current from the event stream while it lives. */
  follow: () => void
}

/**
 * One copy of something several views read (WEB-3).
 *
 * The categories were read by nine views and components, the accounts and proxy profiles by eight,
 * each into a list of its own that nothing kept current: a category created in the settings was
 * missing from the LinkGrabber's picker until the view was opened again. Each now reads one copy.
 *
 * `load()` still asks the server every time — a view that opens shows the stored state, as it
 * did — but the components of one page that mount together share the request. `follow()` re-reads
 * the copy on `events`, debounced, for as long as one following component lives; the last one to
 * go drops the subscription. A read with no event behind it (the settings document) is never
 * followed.
 */
export function sharedRead<T, R extends ApiResult<T>>(
  read: () => Promise<R>,
  initial: T,
  events: readonly string[] = []
): SharedRead<T, R> {
  const value = ref(initial) as Ref<T>
  let inflight: Promise<R> | null = null
  let followers = 0
  // A failed re-read keeps the copy as it was; the next event or load asks again.
  const refresher = debouncedEventRefresh(events, () => load().catch(() => undefined), {
    busy: () => inflight !== null
  })

  function load(): Promise<R> {
    if (inflight) return inflight
    const request = read()
    inflight = request
    // Registered before the caller's own `await`, so the copy is in place when the caller goes on;
    // a rejection reaches the caller through `request`.
    const settle = () => { inflight = null }
    request.then((response) => {
      settle()
      if (response.data !== undefined) value.value = response.data
    }, settle)
    return request
  }

  function follow(): void {
    if (events.length === 0 || !getCurrentScope()) return
    followers += 1
    refresher.connect()
    onScopeDispose(() => {
      followers -= 1
      if (followers === 0) refresher.disconnect()
    })
  }

  return { value, load, follow }
}
