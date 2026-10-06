import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { GRACE_MS, reportServiceReachable, resetServiceConnection, serviceConnection } from '@/composables/serviceConnection'
import {
  onEventStreamOpened,
  resetEventStream,
  resumeEventStream,
  subscribeEvents,
  suspendEventStream
} from '@/composables/useEventStream'

/**
 * Fake `EventSource` recording every instance, so a test can assert how many connections the
 * app opens — the whole point of this module is that there is exactly one.
 */
class FakeEventSource {
  static readonly CONNECTING = 0
  static readonly OPEN = 1
  static readonly CLOSED = 2
  static instances: FakeEventSource[] = []
  listeners = new Map<string, Set<(event: Event) => void>>()
  closed = false
  readyState = FakeEventSource.CONNECTING
  onopen: (() => void) | null = null
  onerror: (() => void) | null = null

  constructor(public url: string) {
    FakeEventSource.instances.push(this)
  }

  addEventListener(name: string, handler: (event: Event) => void): void {
    const set = this.listeners.get(name) ?? new Set()
    set.add(handler)
    this.listeners.set(name, set)
  }

  close(): void {
    this.closed = true
    this.readyState = FakeEventSource.CLOSED
  }

  open(): void {
    this.readyState = FakeEventSource.OPEN
    this.onopen?.()
  }

  /** An error the browser gave up on, as opposed to one it is retrying by itself. */
  fail(): void {
    this.readyState = FakeEventSource.CLOSED
    this.onerror?.()
  }

  emit(name: string, data: string): void {
    for (const handler of this.listeners.get(name) ?? []) {
      handler(new MessageEvent(name, { data }))
    }
  }
}

describe('useEventStream', () => {
  beforeEach(() => {
    FakeEventSource.instances = []
    vi.stubGlobal('EventSource', FakeEventSource)
    vi.useFakeTimers()
  })

  afterEach(() => {
    resetEventStream()
    resetServiceConnection()
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  it('opens a single connection for many subscribers', () => {
    subscribeEvents({ 'a.changed': () => {} })
    subscribeEvents({ 'b.changed': () => {} })
    subscribeEvents({ 'c.changed': () => {} })

    expect(FakeEventSource.instances).toHaveLength(1)
  })

  it('delivers one event to every subscriber of that name', () => {
    const first = vi.fn()
    const second = vi.fn()
    subscribeEvents({ 'usenet.changed': first })
    subscribeEvents({ 'usenet.changed': second })

    FakeEventSource.instances[0]?.emit('usenet.changed', '{}')

    expect(first).toHaveBeenCalledTimes(1)
    expect(second).toHaveBeenCalledTimes(1)
  })

  it('closes the connection once the last subscriber is released', () => {
    const release = subscribeEvents({ 'a.changed': () => {} })
    const other = subscribeEvents({ 'b.changed': () => {} })

    release()
    expect(FakeEventSource.instances[0]?.closed).toBe(false)

    other()
    expect(FakeEventSource.instances[0]?.closed).toBe(true)
  })

  it('leaves no reconnect timer behind after the last release', () => {
    const release = subscribeEvents({ 'a.changed': () => {} })

    FakeEventSource.instances[0]?.fail()
    release()
    vi.advanceTimersByTime(60_000)

    // A pending retry must not resurrect the stream once nobody is listening any more.
    expect(FakeEventSource.instances).toHaveLength(1)
  })

  it('refreshes every subscriber when the server reports dropped events', () => {
    // `stream.lagged` says the bus overwrote events before this connection read them, so every
    // subscriber's view is stale — including the ones that never named the marker.
    const queue = vi.fn()
    const grabber = vi.fn()
    subscribeEvents({ 'download.state': queue, 'download.progress': queue })
    subscribeEvents({ 'collector.changed': grabber })

    FakeEventSource.instances[0]?.emit('stream.lagged', '{"dropped":7}')

    // Once each, although the queue registered its handler under two names.
    expect(queue).toHaveBeenCalledTimes(1)
    expect(grabber).toHaveBeenCalledTimes(1)
  })

  it('stops delivering the lag marker to a released subscriber', () => {
    const handler = vi.fn()
    const release = subscribeEvents({ 'a.changed': handler })
    subscribeEvents({ 'b.changed': () => {} })

    release()
    FakeEventSource.instances[0]?.emit('stream.lagged', '{"dropped":1}')

    expect(handler).not.toHaveBeenCalled()
  })

  it('reconnects while subscribers remain once the browser has given up', () => {
    subscribeEvents({ 'a.changed': () => {} })

    FakeEventSource.instances[0]?.fail()
    vi.advanceTimersByTime(1_000)

    expect(FakeEventSource.instances).toHaveLength(2)
  })

  it('leaves the resume to the browser while it is still reconnecting by itself', () => {
    // An `EventSource` that reconnects on its own sends `Last-Event-ID` and waits the `retry:`
    // the service asked for; a fresh one starts from nothing. So an error the browser is still
    // retrying must not be answered by closing it and opening a second one.
    subscribeEvents({ 'a.changed': () => {} })

    const stream = FakeEventSource.instances[0]
    stream?.onerror?.()
    vi.advanceTimersByTime(60_000)

    expect(FakeEventSource.instances).toHaveLength(1)
    expect(stream?.closed).toBe(false)
  })

  it('refreshes every subscriber when the server could not resume', () => {
    // `stream.expired` says the id this connection resumed from is gone from the buffer —
    // after a restart, or too old — so, like a lag, every view is stale.
    const queue = vi.fn()
    const grabber = vi.fn()
    subscribeEvents({ 'download.state': queue, 'download.progress': queue })
    subscribeEvents({ 'collector.changed': grabber })

    FakeEventSource.instances[0]?.emit('stream.expired', '{"last_event_id":"x"}')

    expect(queue).toHaveBeenCalledTimes(1)
    expect(grabber).toHaveBeenCalledTimes(1)
  })

  it('refreshes every subscriber once a stream it had to reopen is up', () => {
    // A new `EventSource` carries no `Last-Event-ID`, so nothing tells the stores what they missed.
    const queue = vi.fn()
    subscribeEvents({ 'download.state': queue, 'download.progress': queue })

    FakeEventSource.instances[0]?.open()
    expect(queue).not.toHaveBeenCalled()

    FakeEventSource.instances[0]?.fail()
    vi.advanceTimersByTime(1_000)
    FakeEventSource.instances[1]?.open()

    expect(queue).toHaveBeenCalledTimes(1)
  })

  it('stops asking while there is no session', () => {
    subscribeEvents({ 'a.changed': () => {} })

    // The session ended: the service refuses the stream with a 401 and the browser gives up.
    FakeEventSource.instances[0]?.fail()
    suspendEventStream()
    vi.advanceTimersByTime(120_000)
    subscribeEvents({ 'b.changed': () => {} })

    expect(FakeEventSource.instances).toHaveLength(1)
  })

  it('opens at once after the next sign-in, whatever the backoff had reached', () => {
    subscribeEvents({ 'a.changed': () => {} })
    for (let failure = 0; failure < 5; failure += 1) {
      FakeEventSource.instances.at(-1)?.fail()
      vi.advanceTimersByTime(30_000)
    }
    const before = FakeEventSource.instances.length
    suspendEventStream()

    resumeEventStream()

    expect(FakeEventSource.instances).toHaveLength(before + 1)
    // The backoff starts over: the next failure is retried after one second, not thirty.
    FakeEventSource.instances.at(-1)?.fail()
    vi.advanceTimersByTime(1_000)
    expect(FakeEventSource.instances).toHaveLength(before + 2)
  })

  it('reconnects as soon as the browser reports the network back', () => {
    subscribeEvents({ 'a.changed': () => {} })
    for (let failure = 0; failure < 4; failure += 1) {
      FakeEventSource.instances.at(-1)?.fail()
      vi.advanceTimersByTime(30_000)
    }
    FakeEventSource.instances.at(-1)?.fail()
    const before = FakeEventSource.instances.length

    window.dispatchEvent(new Event('online'))

    expect(FakeEventSource.instances).toHaveLength(before + 1)
  })

  it('leaves a stream the browser is still retrying alone when the network returns', () => {
    subscribeEvents({ 'a.changed': () => {} })

    window.dispatchEvent(new Event('online'))

    expect(FakeEventSource.instances).toHaveLength(1)
  })

  it('reports the stream breaking off and coming back to the connection state', () => {
    subscribeEvents({ 'a.changed': () => {} })
    const first = FakeEventSource.instances[0]
    first?.open()
    expect(serviceConnection.value).toBe('connected')

    first?.fail()
    vi.advanceTimersByTime(GRACE_MS)
    expect(serviceConnection.value).toBe('disconnected')

    // The backoff's own retry opens a new stream; its open is the way back.
    vi.advanceTimersByTime(1_000)
    FakeEventSource.instances[1]?.open()
    expect(serviceConnection.value).toBe('connected')
  })

  it('opens a stream waiting out its backoff as soon as a request is answered again', () => {
    subscribeEvents({ 'a.changed': () => {} })
    FakeEventSource.instances[0]?.fail()
    vi.advanceTimersByTime(GRACE_MS)
    expect(FakeEventSource.instances).toHaveLength(2)
    FakeEventSource.instances[1]?.fail()
    vi.advanceTimersByTime(GRACE_MS)

    reportServiceReachable()

    expect(FakeEventSource.instances).toHaveLength(3)
  })

  it('says nothing about the service while there is no session', () => {
    subscribeEvents({ 'a.changed': () => {} })
    FakeEventSource.instances[0]?.fail()
    suspendEventStream()
    vi.advanceTimersByTime(GRACE_MS)
    expect(serviceConnection.value).toBe('connected')
  })

  it('tells its open listeners about every open, the browser\'s own reconnect included', () => {
    // A service restarted by an update shows itself as a reopened stream (RD-1120-16).
    const opened = vi.fn()
    const stop = onEventStreamOpened(opened)
    subscribeEvents({ 'a.changed': () => {} })
    const stream = FakeEventSource.instances[0]

    stream?.open()
    stream?.open()
    expect(opened).toHaveBeenCalledTimes(2)

    stop()
    stream?.open()
    expect(opened).toHaveBeenCalledTimes(2)
  })
})
