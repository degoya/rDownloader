import { describe, expect, it, vi } from 'vitest'

import { type NotifierEnvironment, QUIET_SUMMARY_TAG, createNotifier } from './desktopNotifier'

/** A lock manager two tabs share: the first to ask leads, the next one waits for it. */
function sharedLock() {
  const waiting: Array<() => void> = []
  let held = false
  return {
    lead(onLeader: () => void) {
      if (held) {
        waiting.push(onLeader)
        return
      }
      held = true
      onLeader()
    },
    /** The leading tab closes: the next one in line leads. */
    release() {
      held = false
      const next = waiting.shift()
      if (next) {
        held = true
        next()
      }
    }
  }
}

interface Tab {
  shown: Array<[string, string | undefined, string | undefined]>
  timers: Array<() => void>
}

function tab(overrides: Partial<NotifierEnvironment> = {}, lead?: (onLeader: () => void) => void) {
  const state: Tab = { shown: [], timers: [] }
  const notifier = createNotifier({
    show: async (title, body, tag) => { state.shown.push([title, body, tag]) },
    pushCarries: async () => false,
    quietUntil: async () => null,
    lead: lead ?? (onLeader => onLeader()),
    summary: held => ({ title: `While it was quiet: ${held.length}`, body: held.map(entry => entry.title).join('; ') }),
    now: () => 1_000,
    setTimer: callback => { state.timers.push(callback) },
    ...overrides
  })
  return { notifier, state }
}

/** The counted cases of RD-1260-04 at the browser: two tabs, push on, quiet hours. */
describe('desktop notifier', () => {
  it('shows a message once with two tabs open, from the leading tab', async () => {
    const lock = sharedLock()
    const first = tab({}, onLeader => lock.lead(onLeader))
    const second = tab({}, onLeader => lock.lead(onLeader))
    for (const { notifier } of [first, second]) {
      await notifier.notify('Downloads failed: Holiday', '50 of 50 files could not be downloaded.', { tag: 'package-failed:p' })
    }
    expect(first.state.shown).toEqual([['Downloads failed: Holiday', '50 of 50 files could not be downloaded.', 'package-failed:p']])
    expect(second.state.shown).toEqual([])

    // The leading tab closes; the other one takes over.
    lock.release()
    await second.notifier.notify('Download list finished', '2 files finished.', { tag: 'queue-done' })
    expect(second.state.shown).toHaveLength(1)
  })

  it('leaves to push what push carries to this browser, and nothing else', async () => {
    const pushCarries = vi.fn(async (event: string) => event === 'package_failed')
    const { notifier, state } = tab({ pushCarries })
    await notifier.notify('Downloads failed: Holiday', '50 of 50', { pushEvent: 'package_failed' })
    await notifier.notify('Downloads failed: Holiday', '3 of 50', { tag: 'package-failed:p' })
    expect(state.shown.map(([, body]) => body)).toEqual(['3 of 50'])
  })

  it('holds what arrives in quiet hours and shows it as one summary when they end', async () => {
    const { notifier, state } = tab({ quietUntil: async () => 60_000 })
    for (const name of ['A', 'B', 'C']) await notifier.notify(`Downloads failed: ${name}`)
    expect(state.shown).toEqual([])
    expect(state.timers).toHaveLength(1)

    state.timers[0]?.()
    expect(state.shown).toEqual([
      ['While it was quiet: 3', 'Downloads failed: A; Downloads failed: B; Downloads failed: C', QUIET_SUMMARY_TAG]
    ])
  })

  it('shows a single held message as itself', async () => {
    const { notifier, state } = tab({ quietUntil: async () => 60_000 })
    await notifier.notify('New links', '3 links in the LinkGrabber.', { tag: 'intake' })
    state.timers[0]?.()
    expect(state.shown).toEqual([['New links', '3 links in the LinkGrabber.', undefined]])
  })
})
