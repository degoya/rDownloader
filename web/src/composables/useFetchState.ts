import { ref, type Ref } from 'vue'

/**
 * The state of a data fetch, as the interface has to show it (RD-104-07).
 *
 * A fetched area is in exactly one of three states, and until this composable existed the
 * first and the third were both drawn as the second: while the request was in flight the list
 * was empty, so the empty state rendered — "no accounts", "no downloads" — and when the
 * request failed the list stayed empty, so the same sentence rendered again. Both are false
 * statements, and the second one turns a server failure into "nothing there".
 *
 * `loading` starts `true` on purpose: a component that mounts and immediately fetches is
 * loading from its very first render, not from the moment its `onMounted` handler runs.
 */
export interface FetchState {
  /** True until the first fetch has settled, one way or the other. */
  loading: Ref<boolean>
  /** The failure of the last fetch, in the reader's language, or `null` when it succeeded. */
  loadError: Ref<string | null>
  /**
   * Runs a fetch and records its outcome. `run` resolves to an error message when the fetch
   * failed and to nothing when it succeeded — the shape the existing `refresh()` functions
   * already have, once their `return` carries the message they were only emitting.
   */
  load: (run: () => Promise<string | null | void>) => Promise<void>
}

export function useFetchState(): FetchState {
  const loading = ref(true)
  const loadError = ref<string | null>(null)

  async function load(run: () => Promise<string | null | void>): Promise<void> {
    loading.value = true
    try {
      loadError.value = (await run()) ?? null
    } catch (error) {
      // A thrown request — a dropped connection, a parse failure — is a failed fetch like any
      // other. Swallowing it here would put the caller back in the empty-list-means-nothing
      // trap this composable exists to close.
      loadError.value = error instanceof Error ? error.message : String(error)
    } finally {
      loading.value = false
    }
  }

  return { loading, loadError, load }
}
