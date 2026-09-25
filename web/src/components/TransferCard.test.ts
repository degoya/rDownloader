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

vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))
vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: [] })) },
  responseError: () => 'failed'
}))

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
