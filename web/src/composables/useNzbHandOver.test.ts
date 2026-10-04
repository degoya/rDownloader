/**
 * The NZB hand-over can be switched off in the settings, for the LinkGrabber and the Downloads
 * view each (RD-191-13): every offer reads `targets`, so off empties it for that place alone,
 * while the badge of an NZB already handed over keeps naming its provider.
 */
import { screen, waitFor } from '@testing-library/vue'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'

import type { NzbImport } from '@/api/types'
import NzbImportGroup from '@/components/NzbImportGroup.vue'
import downloads from '@/locales/en/downloads.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import { mountComponent } from '@/test/mount'
import { setShowNzbHandOver } from '@/utils/nzbHandOver'

import { useNzbHandOver } from './useNzbHandOver'

const get = vi.fn(async (path: string) => {
  if (path === '/api/v1/accounts') return { data: [{ id: 'acc-torbox', label: 'Main', provider: 'torbox', enabled: true }] }
  if (path === '/api/v1/remote-jobs/providers') return { data: ['torbox'] }
  if (path === '/api/v1/providers') return { data: [{ slug: 'torbox', display_name: 'TorBox', credentials: 'api_key', kind: 'remote' }] }
  return { data: [] }
})
vi.mock('@/api/client', () => ({
  api: { GET: (path: string) => get(path), POST: vi.fn() },
  responseError: vi.fn()
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))

const handedOver = {
  id: 'nzb-1',
  name: 'Some.Release',
  state: 'imported',
  duplicate: false,
  error: null,
  file_count: 1,
  segment_count: 10,
  total_bytes: '1000',
  position: 1,
  import_mode: 'nzb',
  created_at: '2026-10-04T10:00:00Z',
  handed_over: { remote_job_id: 'job-1', account_id: 'acc-torbox' }
} as unknown as NzbImport

/** One NZB row wired the way `LinkGrabberView` wires it, plus the selection bar's condition. */
const Harness = defineComponent({
  setup() {
    const handOver = useNzbHandOver('linkgrabber')
    return () => h('div', [
      handOver.targets.value.length ? h('button', { 'data-testid': 'grabber-hand-over' }, 'bulk') : null,
      h(NzbImportGroup, {
        item: handedOver,
        categories: [],
        selected: false,
        enqueuing: false,
        deleting: false,
        dragging: false,
        remoteTargets: handOver.targets.value,
        handedOverTo: handOver.handedOverTo(handedOver)
      })
    ])
  }
})

function renderHarness() {
  return mountComponent(Harness, { messages: { linkgrabber, downloads } })
}

afterEach(() => setShowNzbHandOver({}))

describe('useNzbHandOver and the settings switch', () => {
  it('offers the account that takes NZBs while the switch is on', async () => {
    renderHarness()
    await waitFor(() => expect(screen.getByTestId('nzb-hand-over')).toBeTruthy())
    expect(screen.getByTestId('grabber-hand-over')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Main · TorBox' })).toBeTruthy()
    expect(screen.getByTestId('nzb-handed-over').textContent).toBe('Handed to TorBox')
  })

  it('hides the row menu and the bulk entry when off, and keeps the badge', async () => {
    renderHarness()
    await waitFor(() => expect(screen.getByTestId('nzb-hand-over')).toBeTruthy())

    setShowNzbHandOver({ nzb_hand_over_downloads_enabled: true, nzb_hand_over_linkgrabber_enabled: false })

    await waitFor(() => expect(screen.queryByTestId('nzb-hand-over')).toBeNull())
    expect(screen.queryByTestId('grabber-hand-over')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Main · TorBox' })).toBeNull()
    expect(screen.getByTestId('nzb-handed-over').textContent).toBe('Handed to TorBox')
  })

  it('leaves the LinkGrabber alone when only the Downloads switch is off', async () => {
    setShowNzbHandOver({ nzb_hand_over_downloads_enabled: false })
    renderHarness()
    await waitFor(() => expect(screen.getByTestId('nzb-hand-over')).toBeTruthy())
  })

  it('reads a missing value as on, the server default', () => {
    setShowNzbHandOver({ nzb_hand_over_linkgrabber_enabled: null })
    renderHarness()
    return waitFor(() => expect(screen.getByTestId('nzb-hand-over')).toBeTruthy())
  })
})
