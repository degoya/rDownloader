import { nextTick, watch, type ComputedRef, type Ref } from 'vue'
import { useRoute, useRouter, type LocationQuery } from 'vue-router'

/** The row key a `?reveal=` names: `package:<id>` or `file:<id>`, nothing else. */
export function revealKey(value: unknown): string | null {
  return typeof value === 'string' && /^(package|file):[\w-]+$/.test(value) ? value : null
}

interface QueueRevealOptions {
  /** The queue has been read once; before that no row can be found. */
  settled: () => boolean
  /** The package a file belongs to, or nothing for a file the queue does not hold. */
  packageOfFile: (id: string) => string | undefined
  /** The keys of the rows the list shows now. */
  rowKeys: ComputedRef<string[]>
  openPackage: (id: string) => void
  /** Whether the filter or the search hides part of the queue, and the way back from it. */
  filterActive: ComputedRef<boolean>
  resetFilter: () => void
  list: Ref<{ focusRow: (key: string) => Promise<boolean> } | null>
}

/**
 * The jump to one row of the download list (RD-1240-14), as the search palette asks for it with
 * `/downloads?reveal=package:<id>` or `?reveal=file:<id>`. A file's package is opened, a filter
 * that hides the row is reset, and the row takes the keyboard, scrolled into the window; the
 * address then loses `reveal` again, so a reload does not jump a second time. A row the queue
 * no longer holds is simply not found.
 */
export function useQueueReveal(options: QueueRevealOptions): void {
  const route = useRoute()
  const router = useRouter()
  let busy = false

  async function forget(): Promise<void> {
    const query: LocationQuery = { ...route.query }
    delete query.reveal
    await router.replace({ path: route.path, query, hash: route.hash })
  }

  async function reveal(key: string): Promise<void> {
    busy = true
    try {
      if (key.startsWith('file:')) {
        const pkg = options.packageOfFile(key.slice('file:'.length))
        if (pkg) options.openPackage(pkg)
      }
      await nextTick()
      if (!options.rowKeys.value.includes(key) && options.filterActive.value) {
        options.resetFilter()
        await nextTick()
      }
      if (options.rowKeys.value.includes(key)) await options.list.value?.focusRow(key)
      await forget()
    } finally {
      busy = false
    }
  }

  watch(
    () => [revealKey(route.query.reveal), options.settled()] as const,
    ([key, settled]) => {
      if (key && settled && !busy) void reveal(key)
    },
    { immediate: true }
  )
}
