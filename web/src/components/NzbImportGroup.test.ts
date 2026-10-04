/**
 * A failed NZB import opens on its badge (RD-191-11), as a failed package does: the badge said
 * that it failed, and the reason — the import's error and its failed post-processing step —
 * was one chevron away and easy to miss.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import type { NzbImport } from '@/api/types'
import downloads from '@/locales/en/downloads.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import { mountComponent } from '@/test/mount'

import NzbImportGroup from './NzbImportGroup.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn() }
}))

const stepReason = 'Extraction failed: CRC error in Some.Release.part03.rar — the archive is damaged, '
  + 'and the PAR2 volumes did not hold enough recovery blocks to restore it'

function nzb(overrides: Partial<NzbImport> = {}): NzbImport {
  return {
    id: 'nzb-1',
    name: 'Some.Release',
    state: 'failed',
    duplicate: false,
    error: 'The NZB lists articles that no server has any more',
    file_count: 1,
    segment_count: 10,
    total_bytes: '1000',
    position: 1,
    import_mode: 'nzb',
    created_at: '2026-10-04T10:00:00Z',
    ...overrides
  } as unknown as NzbImport
}

function respond(steps: unknown[]): void {
  vi.mocked(api.GET).mockImplementation((async (path: unknown) => {
    return String(path).endsWith('/files')
      ? { data: [{ id: 'f1', subject: 'Some.Release.part01.rar', assembly_name: 'Some.Release.part01.rar', groups: ['alt.binaries'], segments: [], total_bytes: '1000' }] }
      : { data: steps }
  }) as never)
}

function renderGroup(item: NzbImport, extra: Record<string, unknown> = {}) {
  return mountComponent(NzbImportGroup, {
    messages: { linkgrabber, downloads },
    props: { item, categories: [], selected: false, enqueuing: false, deleting: false, dragging: false, ...extra }
  })
}

beforeEach(() => vi.mocked(api.GET).mockReset())

describe('NzbImportGroup failed badge', () => {
  it('opens the group with its error and the failed step reason, and closes again', async () => {
    respond([{ kind: 'extract_rar', state: 'failed', source_path: '/x/Some.Release.part01.rar', message: stepReason, owner_id: 'nzb-1', position: 1, updated_at: '2026-10-04T10:00:00Z' }])
    renderGroup(nzb())
    const badge = screen.getByRole('button', { name: linkgrabber.nzb.state.failed })
    expect(badge.getAttribute('aria-expanded')).toBe('false')
    expect(badge.getAttribute('title')).toBe('The NZB lists articles that no server has any more')

    await fireEvent.click(badge)
    const reason = await screen.findByText(stepReason)
    expect(api.GET).toHaveBeenCalledWith('/api/v1/nzb/imports/{id}/files', expect.anything())
    expect(api.GET).toHaveBeenCalledWith('/api/v1/nzb/imports/{id}/postprocess', expect.anything())
    expect(badge.getAttribute('aria-expanded')).toBe('true')
    expect(screen.getByText('The NZB lists articles that no server has any more')).toBeTruthy()
    expect(screen.getByText('Some.Release.part01.rar')).toBeTruthy()
    expect(reason.className).not.toContain('truncate')
    expect(screen.queryByText(linkgrabber.nzb.failed_no_reason)).toBeNull()

    await fireEvent.click(badge)
    expect(badge.getAttribute('aria-expanded')).toBe('false')
    expect(screen.queryByText(stepReason)).toBeNull()
  })

  it('says so when a failed import carries no reason at all', async () => {
    respond([])
    renderGroup(nzb({ error: null }))
    const badge = screen.getByRole('button', { name: linkgrabber.nzb.state.failed })
    expect(badge.getAttribute('title')).toBe(linkgrabber.nzb.show_files)
    await fireEvent.click(badge)
    expect(await screen.findByText(linkgrabber.nzb.failed_no_reason)).toBeTruthy()
  })

  it('keeps a plain badge for a usable or duplicate import', () => {
    renderGroup(nzb({ state: 'imported', error: null }))
    expect(screen.queryByTestId('nzb-failed')).toBeNull()
    expect(screen.getByText(linkgrabber.nzb.available)).toBeTruthy()
  })
})

/** Handing an NZB to a provider instead of the queue (RD-191-13). */
describe('NzbImportGroup hand-over', () => {
  const targets = [
    { accountId: 'acc-torbox', label: 'Main · TorBox', provider: 'TorBox' },
    { accountId: 'acc-premiumize', label: 'Backup · Premiumize.me', provider: 'Premiumize.me' }
  ]

  it('offers every account that takes NZBs and hands the import to the one picked', async () => {
    const { emitted } = renderGroup(nzb({ state: 'imported', error: null }), { remoteTargets: targets })
    const trigger = screen.getByTestId('nzb-hand-over')
    expect(trigger.getAttribute('aria-label')).toBe(linkgrabber.nzb.hand_over.action)
    expect(trigger.getAttribute('title')).toBe(linkgrabber.nzb.hand_over.hint)
    await fireEvent.click(screen.getByRole('button', { name: 'Backup · Premiumize.me' }))
    expect(emitted().handOver).toEqual([['nzb-1', 'acc-premiumize']])
  })

  it('shows no hand-over where no account takes NZBs', () => {
    renderGroup(nzb({ state: 'imported', error: null }), { remoteTargets: [] })
    expect(screen.queryByTestId('nzb-hand-over')).toBeNull()
  })

  it('keeps a failed import out of the menu', () => {
    respond([])
    renderGroup(nzb(), { remoteTargets: targets })
    expect(screen.queryByTestId('nzb-hand-over')).toBeNull()
  })

  it('marks a handed-over import, leads to the remote jobs and keeps enqueueing possible', () => {
    renderGroup(
      nzb({ state: 'imported', error: null, handed_over: { remote_job_id: 'job-1', account_id: 'acc-torbox' } } as Partial<NzbImport>),
      { remoteTargets: targets, handedOverTo: 'TorBox' }
    )
    const badge = screen.getByTestId('nzb-handed-over')
    expect(badge.textContent).toBe('Handed to TorBox')
    expect(badge.getAttribute('title')).toBe(linkgrabber.nzb.hand_over.badge_hint)
    const enqueue = screen.getByRole('button', { name: linkgrabber.actions.enqueue })
    expect(enqueue.hasAttribute('disabled')).toBe(false)
    expect(enqueue.getAttribute('title')).toBe('Already handed to TorBox – enqueueing downloads the NZB here as well.')
  })

  it('shows no badge for an import that was not handed over', () => {
    renderGroup(nzb({ state: 'imported', error: null }), { remoteTargets: targets })
    expect(screen.queryByTestId('nzb-handed-over')).toBeNull()
    expect(screen.getByRole('button', { name: linkgrabber.actions.enqueue }).getAttribute('title')).toBeNull()
  })
})
