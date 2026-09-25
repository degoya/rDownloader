import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { useTransfersStore } from './transfers'

/** Captures the listeners `connectEvents` registers, so a server event can be replayed. */
class EventSourceStub {
  static listeners = new Map<string, (event: MessageEvent<string>) => void>()

  onerror: (() => void) | null = null

  addEventListener(name: string, listener: (event: MessageEvent<string>) => void): void {
    EventSourceStub.listeners.set(name, listener)
  }

  close(): void {}
}

/**
 * A server event as it actually arrives: the whole envelope, with the event's own data
 * nested under `payload`. Reading the fields from the top level found nothing, so live
 * post-processing progress never reached the UI.
 */
function progressEvent(payload: Record<string, unknown>): MessageEvent<string> {
  return {
    data: JSON.stringify({
      id: '019d0000-0000-7000-8000-0000000000ff',
      kind: 'postprocess_progress',
      occurred_at: '2026-09-02T10:00:00Z',
      payload
    })
  } as MessageEvent<string>
}

describe('transfers store: post-processing progress', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    EventSourceStub.listeners.clear()
    vi.stubGlobal('EventSource', EventSourceStub)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('patches the package from the event envelope payload', () => {
    const store = useTransfersStore()
    store.packages = [
      { id: 'pkg-1', name: 'Release', postprocess: null }
    ] as unknown as typeof store.packages
    store.connectEvents()
    const listener = EventSourceStub.listeners.get('postprocess.progress')
    expect(listener).toBeDefined()

    listener?.(
      progressEvent({ owner_id: 'pkg-1', stage: 'unpack', percent: 42, current: 'part1.rar' })
    )

    expect(store.packages[0]?.postprocess).toEqual({
      stage: 'unpack',
      percent: 42,
      current: 'part1.rar'
    })
    store.disconnectEvents()
  })

  it('ignores an event that carries no owner', () => {
    const store = useTransfersStore()
    store.packages = [
      { id: 'pkg-1', name: 'Release', postprocess: null }
    ] as unknown as typeof store.packages
    store.connectEvents()
    const listener = EventSourceStub.listeners.get('postprocess.progress')

    listener?.(progressEvent({ stage: 'unpack', percent: 10 }))
    listener?.({ data: 'not json' } as MessageEvent<string>)

    expect(store.packages[0]?.postprocess).toBeNull()
    store.disconnectEvents()
  })
})
