import { fireEvent, render, screen, within } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { LinkCandidate } from '@/api/types'
import { useAccountProviders } from '@/composables/useAccountProviders'
import type { ConfirmOptions } from '@/composables/useConfirm'
import common from '@/locales/en/common.json'
import en from '@/locales/en/linkgrabber.json'
import { useCollectorStore } from '@/stores/collector'

import CollectorCandidateRow from './CollectorCandidateRow.vue'

// The row reads which providers have an account; the answer does not matter here.
vi.mock('@/api/client', () => ({
  api: { GET: vi.fn().mockResolvedValue({ data: [] }) },
  responseError: vi.fn(),
  errorMessage: vi.fn()
}))

/** Withdrawing an approval asks first; the test drives the answer. */
const confirmed = vi.fn(async (_options: ConfirmOptions) => true)
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirmed }))

// That same lookup subscribes to `plugin_catalog.changed`, and jsdom has no `EventSource`. What it does
// with the event is pinned in `useAccountProviders.test.ts`; here it only must not open a stream.
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { linkgrabber: en, common } } })

// The row reads torrent details from a store; every case here is a non-torrent link.
beforeEach(() => {
  setActivePinia(createPinia())
  confirmed.mockClear()
  confirmed.mockResolvedValue(true)
})

/** Nuxt UI components are auto-imported in the app; the test only needs their shape. */
const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }
const components = {
  UButton: { template: '<button v-bind="$attrs"><slot /></button>' },
  UCheckbox: passthrough,
  UBadge: passthrough,
  UIcon: passthrough,
  USelect: passthrough,
  /**
   * The dropdown rendered open: its trigger stays in place and its items become real buttons,
   * inside a marker so a test can tell "beside the row" from "under the dots".
   */
  UDropdownMenu: {
    props: ['items'],
    template: `<div><slot />
      <div data-menu-items>
        <button v-for="item in (items ?? []).flat()" :key="item.label" type="button" :disabled="item.disabled" @click="item.onSelect?.()">{{ item.label }}</button>
      </div>
    </div>`
  }
}

function candidate(
  request: LinkCandidate['request'] = undefined,
  replayConsent: LinkCandidate['replay_consent'] = undefined
): LinkCandidate {
  return {
    id: 'candidate-1',
    batch_id: 'batch-1',
    url: 'https://files.example.com/report.pdf',
    state: 'online',
    file_name: 'report.pdf',
    created_at: '2026-09-02T10:00:00Z',
    priority: 'normal',
    position: 1,
    request,
    replay_consent: replayConsent
  } as LinkCandidate
}

const capturedPost: LinkCandidate['request'] = {
  effective_url: 'https://cdn.example.com/a/report.pdf',
  method: 'POST',
  headers: []
}

const grantedConsent: LinkCandidate['replay_consent'] = {
  granted_at: '2026-09-03T08:30:00Z',
  template_hash: 'hash-1',
  approved_origins: ['https://cdn.example.com']
}

function renderRow(value: LinkCandidate) {
  return render(CollectorCandidateRow, {
    props: { candidate: value, selected: false, busy: false },
    global: { plugins: [i18n], components }
  })
}

/** A yt-dlp link: the row shows its variant, its page title and offers the MP3 switch. */
function mediaCandidate(selected = 'best'): LinkCandidate {
  return {
    ...candidate(),
    id: 'candidate-media',
    url: 'https://video.example.com/watch?v=abc',
    provider: 'media',
    file_name: 'Some Talk.mp4',
    media: {
      title: 'Some Talk',
      duration_seconds: 900,
      uploader: 'Someone',
      selected,
      variants: [
        { id: 'best', label: 'Best', kind: 'video', ext: 'mp4', format: 'bv*+ba' },
        { id: 'audio_mp3', label: 'Audio (MP3)', kind: 'audio', ext: 'mp3', format: 'ba' }
      ]
    }
  } as unknown as LinkCandidate
}

describe('CollectorCandidateRow', () => {
  // The row used to carry `draggable`, so every drag across the file name started a reorder and
  // selecting the text was impossible. The gesture belongs to the handle, as in both package rows.
  it('starts a drag from the handle and leaves the row itself selectable', async () => {
    const { container, emitted } = renderRow(candidate())
    const row = container.querySelector('div.group')
    expect(row?.getAttribute('draggable')).toBeNull()

    const handle = screen.getByTitle(/Drag to change the order inside the package/)
    expect(handle.getAttribute('draggable')).toBe('true')
    await fireEvent.dragStart(handle)
    expect(emitted().dragstart).toEqual([['candidate-1']])
  })

  it('moves the link with the arrow keys, so the order is reachable without a mouse', async () => {
    const { emitted } = renderRow(candidate())
    const handle = screen.getByTitle(/Drag to change the order inside the package/)

    await fireEvent.keyDown(handle, { key: 'ArrowUp' })
    await fireEvent.keyDown(handle, { key: 'ArrowDown' })

    expect(emitted().move).toEqual([['candidate-1', -1], ['candidate-1', 1]])
  })

  it('offers no details toggle for links without captured request metadata', () => {
    renderRow(candidate())
    expect(screen.queryByTitle('Details')).toBeNull()
  })

  it('reveals the captured request only after the details toggle is used', async () => {
    const { getByTitle } = renderRow(candidate({
      effective_url: 'https://cdn.example.com/a/report.pdf',
      method: 'GET',
      referrer: 'https://example.com/downloads',
      user_agent: 'Mozilla/5.0',
      content_disposition: 'attachment; filename="report.pdf"',
      headers: [{ name: 'accept', value: '*/*' }]
    }))
    expect(screen.queryByText('Method')).toBeNull()

    getByTitle('Details').click()
    await new Promise(resolve => setTimeout(resolve, 0))

    expect(screen.getByText('Method')).toBeTruthy()
    expect(screen.getByText('https://example.com/downloads')).toBeTruthy()
    expect(screen.getByText('attachment; filename="report.pdf"')).toBeTruthy()
    expect(screen.getByText('accept: */*')).toBeTruthy()
  })

  // Without this the approval is invisible: the enqueue skips the dialog for a candidate that
  // already carries one, so an approval left behind by a cancelled enqueue would send the
  // captured credentials on the next attempt with nothing on screen saying so.
  it('names a granted approval in the row', () => {
    renderRow(candidate(capturedPost, grantedConsent))
    expect(screen.getByTitle('Approved to send credentials')).toBeTruthy()
  })

  it('offers no approval badge for a link nobody approved', () => {
    renderRow(candidate(capturedPost))
    expect(screen.queryByTitle('Approved to send credentials')).toBeNull()
  })

  it('withdraws the approval from the details panel, after asking', async () => {
    const collector = useCollectorStore()
    const revoke = vi.spyOn(collector, 'revokeReplayConsent').mockResolvedValue(true)
    const { getByTitle } = renderRow(candidate(capturedPost, grantedConsent))

    getByTitle('Details').click()
    await new Promise(resolve => setTimeout(resolve, 0))
    getByTitle('Withdraw approval').click()
    await new Promise(resolve => setTimeout(resolve, 0))

    expect(confirmed).toHaveBeenCalledTimes(1)
    expect(confirmed.mock.calls[0]?.[0]?.destructive).toBe(true)
    expect(revoke).toHaveBeenCalledWith('candidate-1')
  })

  it('keeps the approval when the confirmation is declined', async () => {
    confirmed.mockResolvedValue(false)
    const collector = useCollectorStore()
    const revoke = vi.spyOn(collector, 'revokeReplayConsent').mockResolvedValue(true)
    const { getByTitle } = renderRow(candidate(capturedPost, grantedConsent))

    getByTitle('Details').click()
    await new Promise(resolve => setTimeout(resolve, 0))
    getByTitle('Withdraw approval').click()
    await new Promise(resolve => setTimeout(resolve, 0))

    expect(revoke).not.toHaveBeenCalled()
  })
})

/**
 * RD-110-27: the link row spends its width the way the queue rows do.
 *
 * It used to be its own flex line with viewport classes, two loose icon buttons, a spelled-out
 * hoster and a dash where no size was known — while the name, the one thing the row exists to
 * say, was the first thing squeezed. Every saving is asserted by the name it kept rather than
 * by the glyph that replaced it.
 */
describe('CollectorCandidateRow grid cells', () => {
  // The shared grid places nine named cells; a row that quietly loses one stops wrapping. The
  // chevron only exists for a link with something to open, so this one carries a request.
  it('carries every named cell of the shared queue row', () => {
    renderRow(candidate(capturedPost))
    const row = document.querySelector('.queue-row') as HTMLElement
    expect(row).toBeTruthy()
    for (const cell of ['handle', 'select', 'expand', 'name', 'state', 'progress', 'size', 'meta', 'actions']) {
      expect(row.querySelector(`.queue-cell-${cell}`), cell).toBeTruthy()
    }
  })

  // The view hands each link row the frame its package carries down (`linkFrame`); a template
  // with several roots cannot take a class from its parent, so the frame never arrived.
  it('takes the frame class the view hands it onto one root', () => {
    const { container } = render(CollectorCandidateRow, {
      props: { candidate: candidate(), selected: false, busy: false },
      attrs: { class: 'border-x' },
      global: { plugins: [i18n], components }
    })
    expect(container.firstElementChild?.classList.contains('border-x')).toBe(true)
    expect(container.children).toHaveLength(1)
  })
})

describe('CollectorCandidateRow row actions', () => {
  /** Everything in the last cell that is not an item of the dots menu. */
  function controlsBesideTheRow(): HTMLElement[] {
    const cell = document.querySelector('.queue-cell-actions') as HTMLElement
    return [...cell.querySelectorAll('button')].filter(button => !button.closest('[data-menu-items]'))
  }

  function menuItems(): string[] {
    const cell = document.querySelector('.queue-cell-actions') as HTMLElement
    const list = cell.querySelector('[data-menu-items]') as HTMLElement
    return [...list.querySelectorAll('button')].map(button => button.textContent?.trim() ?? '')
  }

  it('keeps at most two controls beside the row: enqueue, and the dots', () => {
    renderRow(mediaCandidate())
    const beside = controlsBesideTheRow()
    expect(beside).toHaveLength(2)
    expect(beside[0]?.getAttribute('aria-label')).toBe(en.actions.enqueue_link)
    expect(beside[1]?.getAttribute('aria-label')).toBe(en.actions.link_actions)
  })

  it('moves delete, rename and the MP3 switch under the dots with their labels intact', () => {
    renderRow(mediaCandidate())
    const labels = menuItems()
    for (const label of [common.actions.rename, en.media.mp3_hint, en.actions.delete_link]) {
      expect(labels).toContain(label)
    }
  })

  it('offers no MP3 switch for a link that is not media', () => {
    renderRow(candidate())
    expect(menuItems()).not.toContain(en.media.mp3_hint)
  })

  it('still deletes and renames the link through the menu it moved into', async () => {
    const { emitted } = renderRow(candidate())
    const cell = document.querySelector('.queue-cell-actions') as HTMLElement
    ;(within(cell).getByText(en.actions.delete_link) as HTMLButtonElement).click()
    ;(within(cell).getByText(common.actions.rename) as HTMLButtonElement).click()
    await Promise.resolve()
    expect(emitted().remove?.[0]).toEqual(['candidate-1'])
    expect(emitted().rename?.[0]).toEqual(['candidate-1'])
  })

  it('switches to MP3 through the menu', async () => {
    const { emitted } = renderRow(mediaCandidate())
    const cell = document.querySelector('.queue-cell-actions') as HTMLElement
    ;(within(cell).getByText(en.media.mp3_hint) as HTMLButtonElement).click()
    await Promise.resolve()
    expect(emitted().variant?.[0]).toEqual(['candidate-media', 'audio_mp3'])
  })

  it('cannot delete a link that is still being added', () => {
    renderRow({ ...candidate(), state: 'resolving' } as LinkCandidate)
    const cell = document.querySelector('.queue-cell-actions') as HTMLElement
    expect((within(cell).getByText(en.actions.delete_link) as HTMLButtonElement).disabled).toBe(true)
  })
})

describe('CollectorCandidateRow glyphs and metadata', () => {
  it('names the media badge although it shows only an icon', () => {
    renderRow(mediaCandidate())
    const badge = screen.getByLabelText(en.media.badge)
    // The word is the name, not the content: that is the width the saving bought.
    expect(badge.textContent?.trim()).toBe('')
    expect(badge.getAttribute('title')).toBe(en.media.badge)
  })

  it('keeps the state as a word in its own cell', () => {
    renderRow(candidate())
    const state = document.querySelector('.queue-cell-state') as HTMLElement
    expect(state.textContent?.trim()).toBe(en.candidate.state.online)
  })

  // RD-110-07: the address answered with a page. The row has to say so in its own words —
  // not as "offline", which claims the file is gone — and must not offer it to the queue.
  it('names an address that answered with a page and refuses to select it', () => {
    renderRow({ ...candidate(), state: 'unresolvable' } as LinkCandidate)
    const state = document.querySelector('.queue-cell-state') as HTMLElement
    expect(state.textContent?.trim()).toBe(en.candidate.state.unresolvable)
    expect(state.textContent?.trim()).not.toBe(en.candidate.state.offline)
    expect(within(state).getByTitle(en.candidate.unresolvable_hint)).toBeTruthy()
    // The checkbox gives way to the inert link glyph, as it does for every state that cannot
    // be queued; the enqueue button beside the row is dead for the same reason.
    expect(screen.queryByLabelText(en.candidate.select)).toBeNull()
    const actions = document.querySelector('.queue-cell-actions') as HTMLElement
    expect((within(actions).getByLabelText(en.actions.enqueue_link) as HTMLButtonElement).disabled).toBe(true)
  })

  // RD-120-18: the same state, the other reason. The site rule resolved correctly and handed
  // over an address no installed plugin claims, so the hint must name that host instead of
  // blaming the address — and it names it as a parameter, never as a sentence from the server.
  it('names the hoster when no resolver is installed for it', () => {
    renderRow({
      ...candidate(),
      url: 'https://www.nfile.cc/abcdef123456',
      state: 'unresolvable',
      error_code: 'collector.check_no_resolver'
    } as LinkCandidate)
    const state = document.querySelector('.queue-cell-state') as HTMLElement
    const expected = en.candidate.no_resolver_hint.replace('{host}', 'nfile.cc')
    expect(expected).toContain('nfile.cc')
    expect(within(state).getByTitle(expected)).toBeTruthy()
    // The old sentence accused the address; it must be gone from this row entirely.
    expect(state.innerHTML).not.toContain(en.candidate.unresolvable_hint)
  })

  it('shows the hoster where the accounting has room for metadata', () => {
    renderRow(candidate())
    const meta = document.querySelector('.queue-cell-meta') as HTMLElement
    expect(meta.textContent?.trim()).toBe('files.example.com')
  })

  it('shows the variant picker of a media link in that same cell, named', () => {
    renderRow(mediaCandidate())
    const meta = document.querySelector('.queue-cell-meta') as HTMLElement
    expect(meta.querySelector(`[aria-label="${en.media.variant}"]`)).toBeTruthy()
  })

  it('shows nothing where the size is not known', () => {
    renderRow(candidate())
    const size = document.querySelector('.queue-cell-size') as HTMLElement
    expect(size.textContent?.trim()).toBe('')
  })

  it('shows the size once it is known', () => {
    renderRow({ ...candidate(), size: '1048576' } as unknown as LinkCandidate)
    const size = document.querySelector('.queue-cell-size') as HTMLElement
    expect(size.textContent?.trim()).toMatch(/1(\.0)? MiB/)
  })
})

// RD-120-36: "cached" is not "online". The provider's cache answer is shown as its own chip,
// with the time it was measured, and never replaces the state word: the link is still online,
// and the cache is a statement that can expire without notice.
describe('CollectorCandidateRow cache answer', () => {
  const cachedAt = '2026-09-24T09:15:00Z'

  it('shows when the provider reported the file as cached, beside the unchanged state', () => {
    renderRow({ ...candidate(), cached_at: cachedAt } as LinkCandidate)
    const chip = screen.getByTestId('candidate-cached')
    expect(chip.textContent).toContain(`${en.cache.label}: `)
    expect(chip.textContent).toContain('2026')
    // The time is in the chip and in its explanation, so the answer's age is visible.
    const title = chip.getAttribute('title') ?? ''
    expect(title).toContain('2026')
    expect(title).toContain((en.cache.hint.split('{at}')[1] ?? '').trim())
    const state = document.querySelector('.queue-cell-state') as HTMLElement
    expect(state.textContent?.trim()).toBe(en.candidate.state.online)
  })

  it('shows no cache chip for a link no provider called cached', () => {
    renderRow(candidate())
    expect(screen.queryByTestId('candidate-cached')).toBeNull()
  })

  // RD-130-11: with more than one provider able to answer, the tooltip names the one that did,
  // under the display name the provider catalogue gives it rather than its slug.
  it('names the provider that answered, by its display name', async () => {
    vi.mocked(api.GET).mockImplementation((async (path: string) =>
      path === '/api/v1/providers'
        ? { data: [{ slug: 'torbox', display_name: 'TorBox', credentials: 'api_key', kind: 'multihoster' }] }
        : { data: [] }) as never)
    await useAccountProviders().refresh()
    renderRow({ ...candidate(), cached_at: cachedAt, cached_by: 'torbox' } as LinkCandidate)
    const title = screen.getByTestId('candidate-cached').getAttribute('title') ?? ''
    expect(title).toContain('TorBox reported on')
    expect(title).toContain('2026')
    expect(title).not.toContain('The provider reported')
  })

  it('keeps the unnamed explanation for an answer without a provider', () => {
    renderRow({ ...candidate(), cached_at: cachedAt } as LinkCandidate)
    const title = screen.getByTestId('candidate-cached').getAttribute('title') ?? ''
    expect(title).toContain('The provider reported on')
  })
})

/** A mirror group as the view hands it down: the chosen member plus the rest. */
function mirrorGroup(source: 'declared' | 'name_and_size' | 'name', onlineCount = 2, pinned = false) {
  const members = [candidate(), { ...candidate(), id: 'candidate-2' }] as LinkCandidate[]
  return { key: 'release', source, chosen: members[0], members, pinned, onlineCount }
}

function renderMirrorRow(props: Record<string, unknown>) {
  return render(CollectorCandidateRow, {
    props: { candidate: candidate(), selected: false, busy: false, ...props },
    global: { plugins: [i18n], components }
  })
}

describe('CollectorCandidateRow as a mirror group (RD-110-19)', () => {
  // The substance of the job: a proposal must not read as a fact. The noun changes, not only
  // the colour, so somebody who cannot see the difference still reads the difference.
  it('names a proposed group differently from a declared one', () => {
    const declared = renderMirrorRow({ mirrorGroup: mirrorGroup('declared') })
    expect(screen.getByText('2 mirrors')).toBeTruthy()
    declared.unmount()
    renderMirrorRow({ mirrorGroup: mirrorGroup('name') })
    expect(screen.getByText('2 possible mirrors')).toBeTruthy()
    expect(screen.queryByText('2 mirrors')).toBeNull()
  })

  it('says what the evidence was, in the badge title', () => {
    renderMirrorRow({ mirrorGroup: mirrorGroup('name') })
    const badge = screen.getByText('2 possible mirrors').closest('[title]') as HTMLElement
    expect(badge.getAttribute('title')).toContain(en.mirror.hint_name)
  })

  it('offers the other mirrors through the chevron pair, with aria-expanded', async () => {
    const toggled: string[] = []
    const { emitted } = renderMirrorRow({ mirrorGroup: mirrorGroup('declared'), mirrorOpen: false })
    const toggle = screen.getByLabelText(en.mirror.expand)
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    await fireEvent.click(toggle)
    for (const [key] of emitted()['toggle-mirror'] as [string][]) toggled.push(key)
    expect(toggled).toEqual(['release'])
  })

  it('names the open state as the act that closes it', () => {
    renderMirrorRow({ mirrorGroup: mirrorGroup('declared'), mirrorOpen: true })
    expect(screen.getByLabelText(en.mirror.collapse).getAttribute('aria-expanded')).toBe('true')
  })

  // RD-101-06: the group says how bad it is and stays queueable all the same.
  it('marks a group whose mirrors are all gone without taking the enqueue away', () => {
    renderMirrorRow({ mirrorGroup: mirrorGroup('declared', 0) })
    expect(screen.getByLabelText(en.mirror.all_offline)).toBeTruthy()
    expect((screen.getByLabelText(en.actions.enqueue_link) as HTMLButtonElement).disabled).toBe(false)
  })

  it('says when the choice was made by hand, and offers to take it back', () => {
    renderMirrorRow({ mirrorGroup: mirrorGroup('declared', 2, true) })
    expect(screen.getByLabelText(en.mirror.pinned)).toBeTruthy()
    const menu = document.querySelector('[data-menu-items]') as HTMLElement
    expect(within(menu).getByText(en.mirror.release)).toBeTruthy()
  })

  // RD-110-34: a proposal offers the way out of itself, and only a proposal does. A declared
  // group and one two sizes agree on are a finding about the source, not something one
  // package may click away, so the entry is absent rather than disabled.
  it('offers to ungroup a proposal and nothing else', async () => {
    const declared = renderMirrorRow({ mirrorGroup: mirrorGroup('declared') })
    expect(within(document.querySelector('[data-menu-items]') as HTMLElement).queryByText(en.mirror.dissolve)).toBeNull()
    declared.unmount()
    const agreed = renderMirrorRow({ mirrorGroup: mirrorGroup('name_and_size') })
    expect(within(document.querySelector('[data-menu-items]') as HTMLElement).queryByText(en.mirror.dissolve)).toBeNull()
    agreed.unmount()
    const { emitted } = renderMirrorRow({ mirrorGroup: mirrorGroup('name') })
    const menu = document.querySelector('[data-menu-items]') as HTMLElement
    await fireEvent.click(within(menu).getByText(en.mirror.dissolve))
    expect(emitted()['dissolve-mirror']).toEqual([['candidate-1']])
  })

  // A member is not a second candidate: the group is what gets queued, selected and reordered.
  it('gives a member row the one action it exists for and none of the group\'s', async () => {
    const { emitted } = renderMirrorRow({ mirrorMember: true })
    expect(screen.queryByLabelText(en.candidate.select)).toBeNull()
    expect(document.querySelector('[data-row-handle]')).toBeNull()
    expect(screen.queryByLabelText(en.actions.enqueue_link)).toBeNull()
    await fireEvent.click(screen.getByLabelText(en.mirror.use))
    expect(emitted()['choose-mirror']).toEqual([['candidate-1', true]])
  })
})

describe('CollectorCandidateRow and hidden hosters (RD-130-21)', () => {
  // JDownloader's "hide links of this hoster", where the hoster is noticed: on its row.
  it('offers to hide the links of its hoster, named', async () => {
    const { emitted } = renderRow(candidate())
    const menu = document.querySelector('[data-menu-items]') as HTMLElement
    await fireEvent.click(within(menu).getByText(en.hosters.hide.replace('{host}', 'files.example.com')))
    expect(emitted()['hide-hoster']).toEqual([['files.example.com']])
  })
})
