import { onMounted, onUnmounted } from 'vue'

import { subscribeEvents, type StreamListener } from '@/composables/useEventStream'

/**
 * Re-reads something when the event stream says it changed, at most once per burst (WEB-05).
 *
 * "Subscribe, debounce 300 ms, drop the timer and the subscription on unmount" was written out
 * fourteen times — seven components, six stores and `useAccountProviders` — and the copies had
 * drifted: one store's `disconnect` left its timer armed, so a refresh fired after the view that
 * wanted it was gone. Refetched rather than patched, and debounced, because the events say only
 * that something changed and installing a package or a burst of queue changes sends several.
 */
interface DebouncedEventRefresh {
  /** Arms the debounce by hand, from a handler that also does something else with the event. */
  schedule: () => void
  /** Subscribes; a second call while subscribed does nothing. */
  connect: () => void
  /** Drops the subscription and a pending timer. */
  disconnect: () => void
}

interface DebouncedEventRefreshOptions {
  /** The debounce, 300 ms unless the caller's events come faster. */
  delayMs?: number
  /**
   * True while a refresh is still on its way: the timer re-arms instead of stacking a second
   * request on one in flight, so a burst of events cannot multiply into parallel round trips.
   */
  busy?: () => boolean
  /** Other events the same subscription carries, with handlers of their own. */
  handlers?: Record<string, StreamListener>
}

/**
 * The manual form, for a store or a module-level cache: the caller connects and disconnects.
 * Inside a store it must be this one — a store's setup runs while the first component that uses
 * it is being set up, and lifecycle hooks registered there would belong to that component.
 */
export function debouncedEventRefresh(
  events: readonly string[],
  refresh: () => unknown,
  options: DebouncedEventRefreshOptions = {}
): DebouncedEventRefresh {
  const delay = options.delayMs ?? 300
  let timer: number | null = null
  let release: (() => void) | null = null

  function schedule(): void {
    if (timer !== null) return
    timer = window.setTimeout(() => {
      timer = null
      if (options.busy?.()) return schedule()
      void refresh()
    }, delay)
  }

  function connect(): void {
    if (release) return
    const handlers: Record<string, StreamListener> = {}
    for (const name of events) handlers[name] = schedule
    Object.assign(handlers, options.handlers)
    if (Object.keys(handlers).length) release = subscribeEvents(handlers)
  }

  function disconnect(): void {
    release?.()
    release = null
    if (timer !== null) {
      window.clearTimeout(timer)
      timer = null
    }
  }

  return { schedule, connect, disconnect }
}

/** The component form: subscribed on mount, and subscription and timer dropped on unmount. */
export function useDebouncedEventRefresh(
  events: readonly string[],
  refresh: () => unknown,
  options: DebouncedEventRefreshOptions = {}
): DebouncedEventRefresh {
  const refresher = debouncedEventRefresh(events, refresh, options)
  onMounted(refresher.connect)
  onUnmounted(refresher.disconnect)
  return refresher
}
