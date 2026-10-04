import { mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'

const handlers = new Map<string, (event: MessageEvent) => void>()
const released = vi.fn()
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (map: Record<string, (event: MessageEvent) => void>) => {
    for (const [name, handler] of Object.entries(map)) handlers.set(name, handler)
    return () => {
      handlers.clear()
      released()
    }
  }
}))

const { debouncedEventRefresh, useDebouncedEventRefresh } = await import('./useDebouncedEventRefresh')

function emit(name: string): void {
  handlers.get(name)?.({ data: '{}' } as MessageEvent)
}

/**
 * The event debounce the components and stores share (WEB-05): a burst costs one read, a read
 * still on its way is not stacked on, and nothing fires once the subscriber is gone.
 */
describe('the debounced event refresh', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    handlers.clear()
    released.mockReset()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('turns a burst of events into one read', () => {
    const refresh = vi.fn()
    const events = debouncedEventRefresh(['plugin.changed', 'plugin_trust.changed'], refresh)
    events.connect()

    emit('plugin.changed')
    emit('plugin_trust.changed')
    emit('plugin.changed')
    vi.advanceTimersByTime(299)
    expect(refresh).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(refresh).toHaveBeenCalledTimes(1)
  })

  it('waits for a read still on its way instead of stacking a second one', () => {
    const refresh = vi.fn()
    let busy = true
    const events = debouncedEventRefresh(['download.state'], refresh, { delayMs: 400, busy: () => busy })
    events.connect()

    emit('download.state')
    vi.advanceTimersByTime(400)
    expect(refresh).not.toHaveBeenCalled()
    busy = false
    vi.advanceTimersByTime(400)
    expect(refresh).toHaveBeenCalledTimes(1)
  })

  it('carries other handlers on the same subscription and drops the pending read on disconnect', () => {
    const refresh = vi.fn()
    const other = vi.fn()
    const events = debouncedEventRefresh(['collector.changed'], refresh, { handlers: { 'collector.intake': other } })
    events.connect()
    events.connect()

    emit('collector.intake')
    expect(other).toHaveBeenCalledTimes(1)
    emit('collector.changed')
    events.disconnect()
    vi.advanceTimersByTime(1_000)

    expect(refresh).not.toHaveBeenCalled()
    expect(released).toHaveBeenCalledTimes(1)
  })

  it('subscribes a component on mount and lets go on unmount', () => {
    const refresh = vi.fn()
    const Probe = defineComponent({
      setup() {
        useDebouncedEventRefresh(['managed_tool.changed'], refresh)
        return () => h('div')
      }
    })

    const wrapper = mount(Probe)
    expect(handlers.has('managed_tool.changed')).toBe(true)
    emit('managed_tool.changed')
    wrapper.unmount()
    vi.advanceTimersByTime(1_000)

    expect(refresh).not.toHaveBeenCalled()
    expect(released).toHaveBeenCalledTimes(1)
  })
})
