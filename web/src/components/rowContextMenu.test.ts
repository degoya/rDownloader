/**
 * The right-click menu of the two lists (RD-1240-14): every row's `UContextMenu` carries the
 * same entries as its dots, so a right-click is a shortcut to what the dots offer and never a
 * second menu that drifts from the first. The stubs record what each menu was handed; the
 * labels are compared group by group.
 */
import { describe, expect, it, vi } from 'vitest'

import type { CollectorPackage, Download, DownloadPackage, LinkCandidate } from '@/api/types'
import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import torrent from '@/locales/en/torrent.json'
import { mountComponent } from '@/test/mount'

import CollectorCandidateRow from './CollectorCandidateRow.vue'
import CollectorPackageGroup from './CollectorPackageGroup.vue'
import PackageGroup from './PackageGroup.vue'
import TransferCard from './TransferCard.vue'

vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(null) }) }) })
}))
vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: [] })) },
  responseError: () => 'failed',
  errorMessage: () => 'failed'
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))

interface MenuItem { label?: string }

/** Both menus of one mount, as the labels of each group. */
function recordingStubs() {
  const seen: { dots: string[][], context: string[][], wrapsRow: boolean } = { dots: [], context: [], wrapsRow: false }
  const labels = (items: MenuItem[][] | undefined) => (items ?? []).map(group => group.map(item => item.label ?? ''))
  return {
    seen,
    stubs: {
      UDropdownMenu: {
        props: ['items'],
        setup(props: { items: MenuItem[][] }) {
          // The dots are a row's last dropdown; a package header's priority menu comes before them.
          seen.dots = labels(props.items)
          return {}
        },
        template: '<div><slot /></div>'
      },
      UContextMenu: {
        props: ['items', 'disabled'],
        setup(props: { items: MenuItem[][] }) {
          seen.context = labels(props.items)
          return {}
        },
        template: '<div data-context-menu><slot /></div>'
      }
    }
  }
}

function wrapsRow(container: Element, row: string): boolean {
  return Boolean(container.querySelector(`[data-context-menu] > ${row}`))
}

describe('the right-click menu of a row', () => {
  it('offers a download row the entries of its dots', () => {
    const { seen, stubs } = recordingStubs()
    const { container } = mountComponent(TransferCard, {
      messages: { downloads, torrent, common },
      props: {
        download: {
          id: 'd1', kind: 'http', state: 'failed', file_name: 'release.rar',
          source: 'https://example.invalid/release.rar', committed_bytes: '0', total_bytes: '100'
        } as unknown as Download
      },
      stubs
    })
    expect(seen.context.flat().length).toBeGreaterThan(2)
    expect(seen.context).toEqual(seen.dots)
    expect(wrapsRow(container, '.queue-row')).toBe(true)
  })

  it('offers a package header of the download list the entries of its dots', () => {
    const { seen, stubs } = recordingStubs()
    const { container } = mountComponent(PackageGroup, {
      messages: { downloads, common },
      props: {
        package: {
          id: 'package-1', name: 'Some Release', state: 'queued', destination: '/downloads/Some Release',
          category_id: null, priority: 'normal', position: 1, has_password: false, kind: 'http',
          nzb_import_id: null, created_at: '2026-09-02T10:00:00Z', enrichment: []
        } as unknown as DownloadPackage,
        downloads: [], categories: [], selection: 'none', open: false, complete: false,
        packageRate: 0, packageEta: null, dragging: false, canPause: false, canResume: false, controlBusy: null
      },
      stubs
    })
    expect(seen.context.flat()).toContain(downloads.package.delete_aria)
    expect(seen.context).toEqual(seen.dots)
    expect(wrapsRow(container, 'header.queue-row')).toBe(true)
  })

  it('offers a LinkGrabber link the entries of its dots', () => {
    const { seen, stubs } = recordingStubs()
    const { container } = mountComponent(CollectorCandidateRow, {
      messages: { linkgrabber, common },
      props: {
        candidate: {
          id: 'candidate-1', batch_id: 'batch-1', url: 'https://files.example.com/report.pdf', state: 'online',
          file_name: 'report.pdf', created_at: '2026-09-02T10:00:00Z', priority: 'normal', position: 1
        } as LinkCandidate,
        selected: false,
        busy: false
      },
      stubs
    })
    expect(seen.context.flat()).toContain(linkgrabber.actions.delete_link)
    expect(seen.context).toEqual(seen.dots)
    expect(wrapsRow(container, '.queue-row')).toBe(true)
  })

  it('offers a LinkGrabber package the entries of its dots', () => {
    const { seen, stubs } = recordingStubs()
    const { container } = mountComponent(CollectorPackageGroup, {
      messages: { linkgrabber, common },
      props: {
        package: { id: 'package-1', name: 'Report', priority: 'normal', has_password: false } as CollectorPackage,
        candidates: [], categories: [], selectedIds: new Set<string>(), enqueuingIds: new Set<string>(),
        dragging: false, open: false
      },
      stubs
    })
    expect(seen.context.flat()).toContain(common.export.action)
    expect(seen.context).toEqual(seen.dots)
    expect(wrapsRow(container, 'header')).toBe(true)
  })
})
