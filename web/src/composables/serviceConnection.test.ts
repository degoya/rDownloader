import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { effectScope, ref } from 'vue'

import {
  clearWhenReconnected,
  failedViewRoute,
  GRACE_MS,
  isChunkLoadError,
  onServiceReconnected,
  reportServiceReachable,
  reportServiceUnreachable,
  reportViewLoadFailure,
  resetServiceConnection,
  resetServiceConnectionForTests,
  serviceConnection,
  takeFailedViewRoute
} from '@/composables/serviceConnection'
import { translateServerMessage } from '@/i18n/server'

describe('serviceConnection', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    resetServiceConnectionForTests()
  })

  afterEach(() => {
    resetServiceConnectionForTests()
    vi.useRealTimers()
  })

  it('shows a loss only once it has stood the grace period', () => {
    reportServiceUnreachable()
    vi.advanceTimersByTime(GRACE_MS - 1)
    expect(serviceConnection.value).toBe('connected')
    vi.advanceTimersByTime(1)
    expect(serviceConnection.value).toBe('disconnected')
  })

  it('ignores a blip the service answers within the grace period', () => {
    const reconnected = vi.fn()
    onServiceReconnected(reconnected)
    reportServiceUnreachable()
    reportServiceReachable()
    vi.advanceTimersByTime(GRACE_MS * 2)
    expect(serviceConnection.value).toBe('connected')
    expect(reconnected).not.toHaveBeenCalled()
  })

  it('announces the way back once, not every answer after it', () => {
    const reconnected = vi.fn()
    onServiceReconnected(reconnected)
    reportServiceUnreachable()
    vi.advanceTimersByTime(GRACE_MS)
    // Further failures while lost restart nothing.
    reportServiceUnreachable()
    reportServiceReachable()
    reportServiceReachable()
    expect(serviceConnection.value).toBe('connected')
    expect(reconnected).toHaveBeenCalledTimes(1)
  })

  it('forgets a pending loss when the stream is closed on purpose', () => {
    reportServiceUnreachable()
    resetServiceConnection()
    vi.advanceTimersByTime(GRACE_MS)
    expect(serviceConnection.value).toBe('connected')
  })

  it('clears the unreachable alert on reconnect and leaves any other message', () => {
    const unreachable = translateServerMessage({ code: 'network.unreachable' })
    expect(unreachable).toMatch(/could not be reached/)
    const transferError = ref<string | null>(`Two refused · ${unreachable}`)
    const otherError = ref<string | null>('Disk full')
    const scope = effectScope()
    scope.run(() => {
      clearWhenReconnected(transferError)
      clearWhenReconnected(otherError)
    })
    reportServiceUnreachable()
    vi.advanceTimersByTime(GRACE_MS)
    reportServiceReachable()
    expect(transferError.value).toBeNull()
    expect(otherError.value).toBe('Disk full')

    // Disposed with its scope: a later outage touches nothing.
    scope.stop()
    transferError.value = unreachable
    reportServiceUnreachable()
    vi.advanceTimersByTime(GRACE_MS)
    reportServiceReachable()
    expect(transferError.value).toBe(unreachable)
  })

  it('remembers a view that failed to load until it is taken once', () => {
    reportViewLoadFailure('/stats?range=day')
    expect(failedViewRoute.value).toBe('/stats?range=day')
    expect(takeFailedViewRoute()).toBe('/stats?range=day')
    expect(takeFailedViewRoute()).toBeNull()
  })

  it('recognises the chunk failure of each browser and nothing else', () => {
    expect(isChunkLoadError(new TypeError('Failed to fetch dynamically imported module: http://x/assets/StatsView.js'))).toBe(true)
    expect(isChunkLoadError(new TypeError('error loading dynamically imported module: http://x/a.js'))).toBe(true)
    expect(isChunkLoadError(new TypeError('Importing a module script failed.'))).toBe(true)
    expect(isChunkLoadError(new TypeError('Failed to fetch'))).toBe(false)
    expect(isChunkLoadError('Importing a module script failed.')).toBe(false)
  })
})
