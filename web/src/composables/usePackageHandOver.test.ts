/**
 * A LinkGrabber package handed to a remote-job provider from its menu (RD-1440-07): the accounts
 * offered are those whose provider takes one of the package's kinds of source, the settings
 * switch hides the entry, and the toast counts what went and what stayed.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock, eventStreamMock, toastMock } from '@/test/mocks'
import { defineComponent, h } from 'vue'

import type { CollectorPackage, LinkCandidate } from '@/api/types'
import CollectorCandidateRow from '@/components/CollectorCandidateRow.vue'
import CollectorPackageGroup from '@/components/CollectorPackageGroup.vue'
import linkgrabber from '@/locales/en/linkgrabber.json'
import { mountComponent, settle } from '@/test/mount'
import { setShowPackageHandOver } from '@/utils/packageHandOver'

import { usePackageHandOver } from './usePackageHandOver'

const { add, submit } = vi.hoisted(() => ({ add: vi.fn(), submit: vi.fn() }))

const get = vi.fn(async (path: string, init?: { params?: { query?: { container?: string } } }) => {
  if (path === '/api/v1/accounts') {
    return {
      data: [
        { id: 'acc-torbox', label: 'Main', provider: 'torbox', enabled: true },
        { id: 'acc-rd', label: 'Second', provider: 'realdebrid', enabled: true },
        { id: 'acc-off', label: 'Off', provider: 'torbox', enabled: false }
      ]
    }
  }
  if (path === '/api/v1/remote-jobs/providers') {
    const container = init?.params?.query?.container
    if (container === 'torrent') return { data: ['realdebrid'] }
    return { data: container === 'nzb' ? ['torbox'] : ['realdebrid', 'torbox'] }
  }
  if (path === '/api/v1/providers') {
    return {
      data: [
        { slug: 'torbox', display_name: 'TorBox', credentials: 'api_key', kind: 'remote' },
        { slug: 'realdebrid', display_name: 'Real-Debrid', credentials: 'api_key', kind: 'remote' }
      ]
    }
  }
  return { data: [] }
})
vi.mock('@/api/client', () => apiClientMock({ GET: (path: string, init?: never) => get(path, init), responseError: vi.fn() }))
vi.mock('@/api/packageHandOver', () => ({ handOverCollectorPackage: (...args: unknown[]) => submit(...args) }))
vi.mock('@/composables/useEventStream', () => eventStreamMock())
vi.mock('@nuxt/ui/composables', () => toastMock({ add }))

/** The menu opened, with every sub-menu's entries as buttons beside their parent. */
const OpenMenu = defineComponent({
  props: { items: { type: Array, default: () => [] } },
  setup(props, { slots }) {
    type Item = { label: string, disabled?: boolean, onSelect?: () => void, children?: Item[] }
    const flat = (items: Item[]): Item[] => items.flatMap(item => [item, ...flat(item.children ?? [])])
    return () => h('div', [
      slots.default?.(),
      ...flat((props.items as Item[][]).flat()).map(item =>
        h('button', { type: 'button', disabled: item.disabled, onClick: () => item.onSelect?.() }, item.label))
    ])
  }
})

const release = {
  id: 'pkg-1',
  name: 'Show.S01',
  batch_id: 'batch-1',
  auto_named: false,
  category_id: null,
  priority: 'normal',
  position: 1,
  has_password: false,
  created_at: '2026-11-21T10:00:00Z'
} as unknown as CollectorPackage

function link(id: string, url: string, provider = 'files.example'): LinkCandidate {
  return { id, url, provider, state: 'online', package_id: 'pkg-1', size: null } as unknown as LinkCandidate
}

function renderPackage(candidates: LinkCandidate[]) {
  const Harness = defineComponent({
    setup() {
      const handOver = usePackageHandOver()
      return () => h(CollectorPackageGroup, {
        package: release,
        candidates,
        categories: [],
        selectedIds: new Set<string>(),
        enqueuingIds: new Set<string>(),
        dragging: false,
        open: false,
        remoteTargets: handOver.targets(candidates),
        onHandOver: (_id: string, accountId: string) => handOver.handOver(release, accountId)
      })
    }
  })
  return mountComponent(Harness, { messages: { linkgrabber }, stubs: { UDropdownMenu: OpenMenu } })
}

const links = [link('c-1', 'magnet:?xt=urn:btih:aaaa'), link('c-2', 'https://files.example/Show.S01E01.mkv')]

afterEach(() => {
  setShowPackageHandOver({})
  add.mockReset()
  submit.mockReset()
})

describe('usePackageHandOver in the package menu', () => {
  it('offers every enabled account that runs remote jobs for a package of links', async () => {
    renderPackage(links)
    await waitFor(() => expect(screen.getByRole('button', { name: 'Main · TorBox' })).toBeTruthy())
    expect(screen.getByRole('button', { name: 'Hand over to remote provider' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Second · Real-Debrid' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Off · TorBox' })).toBeNull()
  })

  it('offers a package of torrent files only to a provider that takes torrents', async () => {
    renderPackage([link('c-3', 'file:///data/torrents/abc.torrent')])
    await waitFor(() => expect(screen.getByRole('button', { name: 'Second · Real-Debrid' })).toBeTruthy())
    expect(screen.queryByRole('button', { name: 'Main · TorBox' })).toBeNull()
  })

  it('offers a package of NZB links only to a provider that takes NZB files', async () => {
    renderPackage([link('c-5', 'https://indexer.example/api?t=get&id=1', 'nzb')])
    await waitFor(() => expect(screen.getByRole('button', { name: 'Main · TorBox' })).toBeTruthy())
    expect(screen.queryByRole('button', { name: 'Second · Real-Debrid' })).toBeNull()
  })

  it('names the indexer\'s own reason for an NZB link that stayed', async () => {
    submit.mockResolvedValue({
      ok: true,
      data: {
        handed_over: [],
        stayed: [{
          candidate_id: 'c-5',
          code: 'collector.nzb_rejected',
          message: 'Request limit reached (code 450)',
          params: { reason: 'Request limit reached (code 450)' }
        }],
        links_removed: true,
        package_removed: false
      }
    })
    renderPackage([link('c-5', 'https://indexer.example/api?t=get&id=1', 'nzb')])
    await fireEvent.click(await screen.findByRole('button', { name: 'Main · TorBox' }))
    await settle()
    expect(add).toHaveBeenCalledWith(expect.objectContaining({
      description: 'The indexer refused to deliver the NZB: Request limit reached (code 450)',
      color: 'warning'
    }))
  })

  it('counts the links handed over in the toast', async () => {
    submit.mockResolvedValue({
      ok: true,
      data: {
        handed_over: [
          { candidate_id: 'c-1', job: { id: 'job-1' }, already_running: false },
          { candidate_id: 'c-2', job: { id: 'job-2' }, already_running: true }
        ],
        stayed: [],
        links_removed: true,
        package_removed: true
      }
    })
    renderPackage(links)
    await fireEvent.click(await screen.findByRole('button', { name: 'Main · TorBox' }))
    await settle()
    expect(submit).toHaveBeenCalledWith('pkg-1', 'acc-torbox')
    expect(add).toHaveBeenCalledWith(expect.objectContaining({ title: '“Show.S01”: 2 links handed to TorBox', color: 'success' }))
  })

  it('names what stayed and why', async () => {
    submit.mockResolvedValue({
      ok: true,
      data: {
        handed_over: [{ candidate_id: 'c-1', job: { id: 'job-1' }, already_running: false }],
        stayed: [{
          candidate_id: 'c-2',
          code: 'collector.remote_job_source_unsupported',
          message: 'Only a magnet, an http(s) address or an imported torrent file can go to a provider'
        }],
        links_removed: true,
        package_removed: false
      }
    })
    renderPackage(links)
    await fireEvent.click(await screen.findByRole('button', { name: 'Second · Real-Debrid' }))
    await settle()
    expect(add).toHaveBeenCalledWith(expect.objectContaining({
      title: '“Show.S01”: 1 of 2 links handed to Real-Debrid, 1 stay in the LinkGrabber',
      description: 'Only a magnet, an http(s) address or an imported torrent file can go to a provider',
      color: 'warning'
    }))
  })

  it('marks a link that stayed after its hand-over with the provider it went to', async () => {
    const marked = { ...link('c-4', 'https://files.example/Show.S01E02.mkv'), handed_over: { remote_job_id: 'job-4', account_id: 'acc-torbox' } } as LinkCandidate
    const Row = defineComponent({
      setup() {
        const handOver = usePackageHandOver()
        return () => h(CollectorCandidateRow, { candidate: marked, selected: false, busy: false, handedOverTo: handOver.handedOverTo(marked) })
      }
    })
    mountComponent(Row, { messages: { linkgrabber } })
    await waitFor(() => expect(screen.getByTestId('candidate-handed-over').textContent).toBe('Handed to TorBox'))
  })

  it('hides the entry when the setting is off', async () => {
    renderPackage(links)
    await waitFor(() => expect(screen.getByRole('button', { name: 'Main · TorBox' })).toBeTruthy())

    setShowPackageHandOver({ linkgrabber_hand_over_enabled: false })

    await waitFor(() => expect(screen.queryByRole('button', { name: 'Hand over to remote provider' })).toBeNull())
    expect(screen.queryByRole('button', { name: 'Main · TorBox' })).toBeNull()
  })
})
