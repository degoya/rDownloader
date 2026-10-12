/**
 * Which tab shows a desktop notification, and when (RD-1260-04).
 *
 * Every open tab watches the queue, and each used to raise its own notification: two tabs, two
 * of each. One tab leads — the browser's lock manager hands the lock to one tab at a time, and to
 * the next once it closes — and only the leader notifies. A message push already carries to this
 * browser is left to push. Quiet hours hold messages back, and their end shows what they held as
 * one notification. The environment is injected so two tabs, push and quiet hours can be driven
 * by a test without a browser.
 */
import type { NotificationEvent } from '@/api/types'

export interface NotifyOptions {
  /** One notification per tag: a later one with the same tag replaces it instead of adding one. */
  tag?: string
  /** The push event that carries the same news; left to push while push reaches this browser. */
  pushEvent?: NotificationEvent
}

export interface HeldNotification {
  title: string
  body?: string
}

export interface NotifierEnvironment {
  show: (title: string, body: string | undefined, tag: string | undefined) => Promise<void>
  /** Whether push reaches this browser with `event`. */
  pushCarries: (event: NotificationEvent) => Promise<boolean>
  /** When the notifications quiet hours hold back go out (epoch ms), or `null` while none are held. */
  quietUntil: () => Promise<number | null>
  /** Asks to become the tab that notifies; `onLeader` runs once this tab is. */
  lead: (onLeader: () => void) => void
  /** What several held notifications say together. */
  summary: (held: readonly HeldNotification[]) => HeldNotification
  now: () => number
  setTimer: (callback: () => void, delayMs: number) => void
}

export interface Notifier {
  notify: (title: string, body?: string, options?: NotifyOptions) => Promise<void>
  isLeader: () => boolean
}

/** The tag of the one notification that reports what quiet hours held back. */
export const QUIET_SUMMARY_TAG = 'quiet-summary'

export function createNotifier(environment: NotifierEnvironment): Notifier {
  let leader = false
  environment.lead(() => { leader = true })
  const held: HeldNotification[] = []
  let flushScheduled = false

  function flush(): void {
    flushScheduled = false
    const batch = held.splice(0)
    const [only] = batch
    if (!only) return
    if (batch.length === 1) {
      void environment.show(only.title, only.body, undefined)
      return
    }
    const summary = environment.summary(batch)
    void environment.show(summary.title, summary.body, QUIET_SUMMARY_TAG)
  }

  async function notify(title: string, body?: string, options: NotifyOptions = {}): Promise<void> {
    if (!leader) return
    if (options.pushEvent && await environment.pushCarries(options.pushEvent)) return
    const until = await environment.quietUntil()
    const now = environment.now()
    if (until !== null && until > now) {
      held.push(body === undefined ? { title } : { title, body })
      if (!flushScheduled) {
        flushScheduled = true
        environment.setTimer(flush, until - now)
      }
      return
    }
    await environment.show(title, body, options.tag)
  }

  return { notify, isLeader: () => leader }
}
