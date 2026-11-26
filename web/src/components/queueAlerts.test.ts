/**
 * RD-1480-18: the alerts above the queue read when they open and when the service announces a
 * change; they polled every five seconds in every open tab.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Component } from 'vue'
import { apiClientMock, eventStreamMock } from '@/test/mocks'

import downloads from '@/locales/en/downloads.json'
import { mountComponent, passTime } from '@/test/mount'

const get = vi.fn()
vi.mock('@/api/client', () => apiClientMock({ GET: (...args: unknown[]) => get(...args) }))
const handlers: Record<string, (event: MessageEvent) => void> = {}
vi.mock('@/composables/useEventStream', () => eventStreamMock({
  subscribeEvents: (registered: Record<string, (event: MessageEvent) => void>) => {
    Object.assign(handlers, registered)
    return () => {}
  }
}))

const { default: StorageCapacityAlert } = await import('./StorageCapacityAlert.vue')
const { default: PowerCountdownAlert } = await import('./power/PowerCountdownAlert.vue')

describe('the alerts above the queue', () => {
  beforeEach(() => {
    get.mockReset()
    get.mockResolvedValue({ data: { roots: [], pending: null } })
  })

  it.each([
    ['storage.capacity', '/api/v1/storage/capacity', StorageCapacityAlert],
    ['power.changed', '/api/v1/power/status', PowerCountdownAlert]
  ] as [string, string, Component][])('follow %s instead of polling', async (event, path, component) => {
    // Mounted on the fake timers, so a poll it armed would run out here.
    await passTime(30_000, () => mountComponent(component, { messages: { downloads } }))
    const asked = (): number => get.mock.calls.filter(call => call[0] === path).length
    expect(asked()).toBe(1)
    await passTime(300, () => handlers[event]?.(new MessageEvent(event, { data: '{}' })))
    expect(asked()).toBe(2)
  })
})
