/**
 * One surface per intake (RD-1260-04): while a capture agent is connected it shows the desktop
 * toast, the service says so on the event, and the tab stays quiet; without one the tab shows it.
 * A hand-over the browser extension announces itself is the extension's alone (RD-1270-05).
 */
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { setLocale } from '@/i18n'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: [] })), POST: vi.fn(), PUT: vi.fn(), PATCH: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => ''),
  resultMessage: vi.fn(() => '')
}))
const handlers: Record<string, (event: MessageEvent) => void> = {}
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (registered: Record<string, (event: MessageEvent) => void>) => {
    Object.assign(handlers, registered)
    return () => {}
  }
}))
const { notify } = vi.hoisted(() => ({ notify: vi.fn() }))
vi.mock('@/composables/useNotifications', () => ({ useNotifications: () => ({ notify }) }))

const { useCollectorStore } = await import('./collector')

function intake(payload: Record<string, unknown>): void {
  handlers['collector.intake']?.(new MessageEvent('collector.intake', { data: JSON.stringify({ payload }) }))
}

describe('collector store: intake toast', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    void setLocale('en')
    notify.mockReset()
    useCollectorStore().connectEvents()
  })

  it('leaves the toast to a connected agent', () => {
    intake({ candidate_count: 8, package_count: 1, agent_toast: true })
    expect(notify).not.toHaveBeenCalled()
  })

  it('leaves a hand-over of the browser extension to the extension, agent or not', () => {
    intake({ candidate_count: 3, package_count: 1, agent_toast: false, extension_notice: true })
    intake({ candidate_count: 3, package_count: 1, agent_toast: true, extension_notice: true })
    expect(notify).not.toHaveBeenCalled()
    intake({ candidate_count: 3, package_count: 1, agent_toast: false, extension_notice: false })
    expect(notify).toHaveBeenCalledTimes(1)
  })

  it('shows one toast per intake without an agent, replacing the last one', () => {
    intake({ candidate_count: 8, package_count: 1, agent_toast: false })
    intake({ candidate_count: 2, package_count: 1 })
    expect(notify).toHaveBeenCalledTimes(2)
    expect(notify).toHaveBeenLastCalledWith('New links', '2 links in the LinkGrabber.', { tag: 'intake' })
  })
})
