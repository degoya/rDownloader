/**
 * Which actions a row offers, per state.
 *
 * Eight predicates spread across this card decide the menu, and both ways of getting one wrong
 * are silent: an action that is offered and cannot work reports a failure the reader did not
 * cause, and one that is withheld leaves a stuck transfer with no way out of the interface.
 * The menu itself is a Nuxt UI dropdown, so the labels are read off the items it is given.
 */
import { describe, expect, it, vi } from 'vitest'

import type { Download, DownloadState } from '@/api/types'
import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import torrent from '@/locales/en/torrent.json'
import { mountComponent } from '@/test/mount'

import TransferCard from './TransferCard.vue'

vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(null) }) }) })
}))
vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: [] })) },
  responseError: () => 'failed'
}))

/** Which host a staged resolver version would handle; none unless a test sets one. */
const staged = vi.hoisted(() => ({ host: null as string | null, trial: null as unknown }))
vi.mock('@/composables/useStagedResolvers', () => {
  staged.trial = vi.fn(async () => ({ message: 'pinned', error: null }))
  return {
    useStagedResolvers: () => ({
      stagedFor: (source: string) => staged.host && source.includes(staged.host)
        ? { pluginId: 'p1', name: 'Hoster', version: '2.0.0', domains: [staged.host] }
        : null,
      trial: staged.trial
    })
  }
})

interface MenuItem { label: string }

/** The labels the dropdown was handed, flattened out of its groups. */
function actionsFor(state: DownloadState): string[] {
  const labels: string[] = []
  mountComponent(TransferCard, {
    messages: { downloads, torrent, common },
    props: {
      download: {
        id: 'd1', kind: 'http', state, file_name: 'release.rar',
        source: 'https://example.invalid/release.rar',
        committed_bytes: '0', total_bytes: '100'
      } as unknown as Download
    },
    stubs: {
      UDropdownMenu: {
        props: ['items'],
        setup(props: { items: MenuItem[][] }) {
          labels.push(...props.items.flat().map(item => item.label))
          return () => null
        }
      }
    }
  })
  return labels
}

interface LinkItem { label: string, to?: string, target?: string, onSelect?: () => void }

/** The menu items for one download, so a case can read `to`/`target` and run `onSelect`. */
function itemsFor(download: Record<string, unknown>): { items: LinkItem[], emitted: () => Record<string, unknown[][]> } {
  const items: LinkItem[] = []
  const view = mountComponent(TransferCard, {
    messages: { downloads, torrent, common },
    props: {
      download: {
        id: 'd1', kind: 'http', state: 'queued', file_name: 'release.rar',
        source: 'https://example.invalid/release.rar',
        committed_bytes: '0', total_bytes: '100',
        ...download
      } as unknown as Download
    },
    stubs: {
      UDropdownMenu: {
        props: ['items'],
        setup(props: { items: LinkItem[][] }) {
          items.push(...props.items.flat())
          return () => null
        }
      }
    }
  })
  return { items, emitted: () => view.emitted() as Record<string, unknown[][]> }
}

/** "Copy link" and "Open source page" (RD-190-21): offered for what the row's data has. */
describe('TransferCard link actions', () => {
  it('copies the row\'s own address in every state', () => {
    for (const state of ['queued', 'downloading', 'failed', 'completed', 'seeding'] as const) {
      expect(actionsFor(state)).toContain(common.actions.copy_link)
    }
    const { items, emitted } = itemsFor({})
    items.find(item => item.label === common.actions.copy_link)?.onSelect?.()
    expect(emitted().copyLinks).toEqual([[['https://example.invalid/release.rar']]])
  })

  it('opens the media page in a new tab, and offers nothing without one', () => {
    expect(itemsFor({}).items.map(item => item.label)).not.toContain(common.actions.open_source_page)

    const { items } = itemsFor({ kind: 'media', media: { page_url: 'https://video.example/watch?v=1' } })
    const open = items.find(item => item.label === common.actions.open_source_page)
    expect(open).toMatchObject({ to: 'https://video.example/watch?v=1', target: '_blank' })
  })
})

describe('TransferCard actions', () => {
  it('offers pause while running and start once it has stopped', () => {
    expect(actionsFor('downloading')).toContain(common.actions.pause)
    expect(actionsFor('downloading')).not.toContain(common.actions.start)

    expect(actionsFor('paused')).toContain(common.actions.start)
    expect(actionsFor('paused')).not.toContain(common.actions.pause)
  })

  /** Renaming a file that is being written would rename it out from under the writer. */
  it('withholds rename while the transfer is running', () => {
    expect(actionsFor('downloading')).not.toContain(common.actions.rename)
    expect(actionsFor('failed')).toContain(common.actions.rename)
  })

  /** Removing a row whose transfer is still running leaves the work without an owner. */
  it('withholds remove while the transfer is running', () => {
    for (const state of ['downloading', 'resolving', 'verifying', 'repairing', 'extracting', 'seeding'] as const) {
      expect(actionsFor(state)).not.toContain(downloads.transfer.remove_aria)
    }
    expect(actionsFor('completed')).toContain(downloads.transfer.remove_aria)
  })

  it('offers no cancel for work that has already stopped for good', () => {
    for (const state of ['completed', 'cancelled', 'seeding'] as const) {
      expect(actionsFor(state)).not.toContain(common.actions.cancel)
    }
    expect(actionsFor('downloading')).toContain(common.actions.cancel)
  })

  /** Seeding stops on its own terms; cancelling it would be a different thing entirely. */
  it('offers stopping the seed only while seeding', () => {
    expect(actionsFor('seeding')).toContain(downloads.transfer.stop_seeding)
    expect(actionsFor('completed')).not.toContain(downloads.transfer.stop_seeding)
  })

  /** "Test with new version" (RD-140-02): only where a loaded staged resolver handles the source. */
  it('offers a trial of the staged resolver version only on a download that is not running', () => {
    const label = downloads.transfer.trial_staged.replace('{name}', 'Hoster').replace('{version}', '2.0.0')
    expect(actionsFor('paused')).not.toContain(label)
    staged.host = 'example.invalid'
    try {
      expect(actionsFor('paused')).toContain(label)
      expect(actionsFor('failed')).toContain(label)
      expect(actionsFor('downloading')).not.toContain(label)
      expect(actionsFor('completed')).not.toContain(label)
    } finally {
      staged.host = null
    }
  })

  it('leaves a completed row nothing to start or pause', () => {
    const labels = actionsFor('completed')
    expect(labels).not.toContain(common.actions.pause)
    expect(labels).not.toContain(common.actions.start)
  })
})

/**
 * The file row and the package header above it share one grid, and since RD-109-30 that grid
 * places nine *named* cells rather than trusting source order — which is what lets the
 * narrowest tier put the state on a second line instead of overrunning the container. A row
 * that stops carrying a cell stops wrapping and starts overlapping, silently.
 */
describe('TransferCard grid cells', () => {
  function renderRow(state: DownloadState, committed = '0') {
    return mountComponent(TransferCard, {
      messages: { downloads, torrent, common },
      props: {
        download: {
          id: 'd1', kind: 'http', state, file_name: 'release.rar',
          source: 'https://example.invalid/release.rar',
          committed_bytes: committed, total_bytes: '100'
        } as unknown as Download
      },
      stubs: { UDropdownMenu: { template: '<div><slot /></div>' } }
    })
  }

  it('carries every named cell of the shared queue row', () => {
    renderRow('downloading')
    const row = document.querySelector('.queue-row') as HTMLElement
    for (const cell of ['handle', 'select', 'expand', 'name', 'state', 'progress', 'size', 'meta', 'actions']) {
      expect(row.querySelector(`.queue-cell-${cell}`), cell).toBeTruthy()
    }
  })

  it('drops the percentage beside a full bar, which says the same thing', () => {
    renderRow('completed', '100')
    expect((document.querySelector('.queue-cell-progress') as HTMLElement).textContent?.trim()).toBe('')
  })

  it('keeps the percentage while the bar is short of the end', () => {
    renderRow('downloading', '40')
    expect((document.querySelector('.queue-cell-progress') as HTMLElement).textContent?.trim()).toBe('40%')
  })
})

/** RD-191-12: when a file is tried again on its own, the card says at what time. */
describe('TransferCard next attempt', () => {
  function card(state: DownloadState, nextRetryAt: string | null) {
    return mountComponent(TransferCard, {
      messages: { downloads, torrent, common },
      props: {
        download: {
          id: 'd1', kind: 'http', state, file_name: 'release.rar',
          source: 'https://example.invalid/release.rar',
          committed_bytes: '0', total_bytes: '100', next_retry_at: nextRetryAt
        } as unknown as Download
      }
    })
  }
  const inAnHour = new Date(Date.now() + 60 * 60 * 1000).toISOString()

  it('shows the time of the retry a waiting file and an automatically retried failed one wait for', () => {
    for (const state of ['retry_wait', 'failed'] as const) {
      const view = card(state, inAnHour)
      const line = view.getByTestId('next-attempt').textContent ?? ''
      expect(line).toContain('Next attempt at')
      expect(line).toMatch(/\d/)
      view.unmount()
    }
  })

  it('says nothing without a due time, or once the file is queued again', () => {
    const failed = card('failed', null)
    expect(failed.queryByTestId('next-attempt')).toBeNull()
    failed.unmount()
    const queued = card('queued', inAnHour)
    expect(queued.queryByTestId('next-attempt')).toBeNull()
  })
})

/** Recheck and change location (RD-1100-10): torrents only, and a move only where no runner writes. */
describe('TransferCard torrent data actions', () => {
  const labels = (download: Record<string, unknown>): string[] =>
    itemsFor({ kind: 'torrent', source: 'magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567', ...download })
      .items.map(item => item.label)

  it('offers a recheck and a move to a seed and to a paused torrent', () => {
    for (const state of ['seeding', 'paused'] as const) {
      expect(labels({ state })).toContain(torrent.actions.recheck)
      expect(labels({ state })).toContain(torrent.actions.move)
    }
  })

  it('rechecks a running torrent but does not move it', () => {
    expect(labels({ state: 'downloading' })).toContain(torrent.actions.recheck)
    expect(labels({ state: 'downloading' })).not.toContain(torrent.actions.move)
  })

  it('offers neither for a finished torrent or a download that is no torrent', () => {
    expect(labels({ state: 'completed' })).not.toContain(torrent.actions.recheck)
    expect(labels({ state: 'completed' })).not.toContain(torrent.actions.move)
    const plain = itemsFor({ state: 'paused' }).items.map(item => item.label)
    expect(plain).not.toContain(torrent.actions.recheck)
    expect(plain).not.toContain(torrent.actions.move)
  })
})
