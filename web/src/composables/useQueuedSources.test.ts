/**
 * What the LinkGrabber learns about its addresses from the duplicate lookup: how often the queue
 * holds the same source (RD-150-01) and, while the setting asks for it, which package of the
 * download history had it (RD-1240-14) — and that a link row says so.
 */
import { screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { LinkCandidate } from '@/api/types'
import common from '@/locales/en/common.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import { mountComponent, passthrough } from '@/test/mount'

import CollectorCandidateRow from '@/components/CollectorCandidateRow.vue'

import { downloadedBefore, queuedCount, refreshQueuedSources } from './useQueuedSources'

const lookup = vi.hoisted(() => vi.fn())
vi.mock('@/api/storage', () => ({ lookupDuplicates: lookup }))
vi.mock('@/api/client', () => ({ api: { GET: vi.fn(async () => ({ data: [] })) }, responseError: () => 'failed', errorMessage: () => 'failed' }))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))

const HOLIDAY = 'https://files.example.com/holiday.zip'
const OTHER = 'https://files.example.com/other.zip'

function answer(history: unknown[] = []) {
  return {
    ok: true,
    data: [
      { url: HOLIDAY, identity: {}, queue: [{}, {}], history },
      { url: OTHER, identity: {}, queue: [], history: [] }
    ]
  }
}

const EARLIER = { history_id: 4, name: 'Holiday Pictures', outcome: 'completed', finished_at: '2026-10-01T09:00:00Z' }

describe('useQueuedSources', () => {
  it('counts the queue and keeps the newest history package per address', async () => {
    lookup.mockResolvedValueOnce(answer([EARLIER, { ...EARLIER, history_id: 2, name: 'Older' }]))
    await refreshQueuedSources([HOLIDAY, OTHER, HOLIDAY])
    expect(lookup).toHaveBeenLastCalledWith([HOLIDAY, OTHER])
    expect(queuedCount(HOLIDAY)).toBe(2)
    expect(queuedCount(OTHER)).toBe(0)
    expect(downloadedBefore(HOLIDAY)?.name).toBe('Holiday Pictures')
    expect(downloadedBefore(OTHER)).toBeNull()

    // The setting switched off: the next answer names no history, and the mark goes.
    lookup.mockResolvedValueOnce(answer())
    await refreshQueuedSources([HOLIDAY, OTHER])
    expect(downloadedBefore(HOLIDAY)).toBeNull()
  })

  it('marks a link row whose source the history holds, naming the package', async () => {
    lookup.mockResolvedValueOnce(answer([EARLIER]))
    await refreshQueuedSources([HOLIDAY])
    mountComponent(CollectorCandidateRow, {
      messages: { linkgrabber, common },
      props: {
        candidate: {
          id: 'candidate-1', batch_id: 'batch-1', url: HOLIDAY, state: 'online', file_name: 'holiday.zip',
          created_at: '2026-09-02T10:00:00Z', priority: 'normal', position: 1
        } as LinkCandidate,
        selected: false,
        busy: false
      },
      stubs: { UBadge: passthrough }
    })
    const badge = screen.getByTestId('history-badge')
    expect(badge.getAttribute('aria-label')).toBe(linkgrabber.duplicates.history)
    expect(badge.getAttribute('title')).toContain('“Holiday Pictures”')
  })

  it('leaves the history mark off an address still in the list, which its own mark says (owner, 2026-10-10)', async () => {
    lookup.mockResolvedValueOnce(answer([EARLIER]))
    await refreshQueuedSources([HOLIDAY])
    mountComponent(CollectorCandidateRow, {
      messages: { linkgrabber, common },
      props: {
        candidate: {
          id: 'candidate-1', batch_id: 'batch-1', url: HOLIDAY, state: 'duplicate', file_name: 'holiday.zip',
          created_at: '2026-09-02T10:00:00Z', priority: 'normal', position: 1
        } as LinkCandidate,
        selected: false,
        busy: false
      },
      stubs: { UBadge: passthrough }
    })
    expect(screen.queryByTestId('history-badge')).toBeNull()
    // Nor the queue's count: the live test of 1.24 showed it beside the orange state (RD-1240-28).
    expect(screen.queryByTestId('queued-badge')).toBeNull()
  })

  it('counts the queued copies of a link that is not marked a duplicate', async () => {
    lookup.mockResolvedValueOnce(answer())
    await refreshQueuedSources([HOLIDAY])
    mountComponent(CollectorCandidateRow, {
      messages: { linkgrabber, common },
      props: {
        candidate: {
          id: 'candidate-1', batch_id: 'batch-1', url: HOLIDAY, state: 'online', file_name: 'holiday.zip',
          created_at: '2026-09-02T10:00:00Z', priority: 'normal', position: 1
        } as LinkCandidate,
        selected: false,
        busy: false
      },
      stubs: { UBadge: passthrough }
    })
    expect(screen.getByTestId('queued-badge')).toBeTruthy()
  })
})
