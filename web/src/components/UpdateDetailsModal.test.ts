/**
 * The update details' self-update (RD-180-02): "Install and restart" behind a confirmation, the
 * install followed through a restart that leaves the service silent for a while, the new version
 * or the old one back with its reason, and a refusal for running downloads asked once more. And
 * the download ahead of the install, in the background with its progress (owner, 2026-10-01), and
 * what the dialog shows when the update fails or the service never comes back (live test
 * 2026-10-01: it said "waiting for it to answer" for good).
 */
import { fireEvent, screen } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { UpdateDownload, UpdateInstall, UpdateOffer, UpdateStatus } from '@/api/updates'
import server from '@/locales/en/server.json'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

const fetchUpdateStatus = vi.fn()
const installUpdate = vi.fn()
const downloadUpdate = vi.fn()
const confirm = vi.fn()
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirm }))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))
vi.mock('@/api/updates', async (original) => ({
  ...(await original<typeof import('@/api/updates')>()),
  fetchUpdateStatus: () => fetchUpdateStatus(),
  installUpdate: (allowActive?: boolean) => installUpdate(allowActive),
  downloadUpdate: () => downloadUpdate()
}))

const { default: UpdateDetailsModal } = await import('./UpdateDetailsModal.vue')
const { useUpdateStatus, FOLLOW_INTERVAL_MS, FOLLOW_LIMIT_MS, DOWNLOAD_INTERVAL_MS } = await import('@/composables/useUpdateStatus')

const modal = {
  props: ['open', 'title'],
  template: '<div v-if="open" data-testid="modal"><h2>{{ title }}</h2><slot name="body" /><slot name="footer" /></div>'
}

const offer: UpdateOffer = {
  version: '1.8.0',
  channel: 'stable',
  released_at: '2026-10-10T12:00:00Z',
  notes: '',
  release_url: 'https://github.com/degoya/rDownloader/releases/tag/v1.8.0',
  changelog_url: 'https://github.com/degoya/rDownloader/blob/v1.8.0/CHANGELOG.md#180---2026-10-10',
  action: 'install',
  command: null,
  hint: null,
  download_url: 'https://github.com/degoya/rDownloader/releases/download/v1.8.0/rdownloader-linux-x86_64.tar.gz',
  download_size: 4096,
  download_sha256: 'ab'.repeat(32),
  rollback_available: true
}

function install(state: UpdateInstall['state'], reason: string | null = null): UpdateInstall {
  return {
    state,
    from_version: '1.8.0-beta.2',
    target_version: '1.8.0',
    reason,
    started_at: '2026-10-10T13:00:00Z',
    updated_at: '2026-10-10T13:00:05Z'
  }
}

function downloaded(state: UpdateDownload['state'], received = 4096, reason: string | null = null): UpdateDownload {
  return { version: '1.8.0', state, received_bytes: received, total_bytes: 4096, reason }
}

function status(patch: Partial<UpdateStatus> = {}): UpdateStatus {
  return {
    current_version: '1.8.0-beta.2',
    configured: true,
    check_enabled: true,
    channel: 'stable',
    effective_channel: 'stable',
    interval_hours: 24,
    install_kind: 'portable',
    checking: false,
    last_checked_at: '2026-10-10T13:00:00Z',
    next_check_at: null,
    error_code: null,
    available: offer,
    install: null,
    download: null,
    capture_agents: [],
    ...patch
  }
}

function mount(shown: UpdateOffer = offer, kind: UpdateStatus['install_kind'] = 'portable') {
  return mountComponent(UpdateDetailsModal, {
    props: { open: true, offer: shown, kind },
    messages: { system, server },
    stubs: { UModal: modal, ULink: { template: '<a v-bind="$attrs"><slot /></a>' } }
  })
}

/** Lets a follow that still runs end, so the next test's own is not taken for it. */
async function settleFollows(): Promise<void> {
  fetchUpdateStatus.mockResolvedValue({ ok: true, data: status({ install: install('done') }) })
  await vi.advanceTimersByTimeAsync(FOLLOW_INTERVAL_MS)
  vi.useRealTimers()
}

async function clickInstall(): Promise<void> {
  await fireEvent.click(screen.getByTestId('update-install-start'))
  await vi.waitFor(() => expect(installUpdate).toHaveBeenCalled())
}

describe('UpdateDetailsModal: installing', () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    fetchUpdateStatus.mockReset()
    installUpdate.mockReset()
    downloadUpdate.mockReset()
    confirm.mockReset()
    const shared = useUpdateStatus()
    shared.status.value = status()
    shared.installFailure.value = null
    shared.downloadFailure.value = null
    shared.followed.value = false
    shared.lost.value = false
  })
  afterEach(async () => {
    await settleFollows()
  })

  it('offers the install with its automatic way back', () => {
    mount()
    expect(screen.getByTestId('update-install').textContent).toContain(system.updates.modal.install_hint)
    expect(screen.getByTestId('update-install-rollback').textContent).toContain(system.updates.modal.rollback_automatic)
    expect(screen.getByRole('button', { name: system.updates.modal.install })).toBeTruthy()
    expect(screen.getByRole('button', { name: system.updates.modal.download })).toBeTruthy()
  })

  it('says so when a Windows installer update cannot be taken back', () => {
    mount({ ...offer, rollback_available: false }, 'msi')
    expect(screen.getByTestId('update-install-rollback').textContent).toContain(system.updates.modal.rollback_unavailable)
  })

  it('installs nothing without the confirmation', async () => {
    confirm.mockResolvedValue(false)
    mount()
    await fireEvent.click(screen.getByTestId('update-install-start'))
    await vi.waitFor(() => expect(confirm).toHaveBeenCalled())
    expect(installUpdate).not.toHaveBeenCalled()
  })

  it('follows the install through the restart to the new version', async () => {
    confirm.mockResolvedValue(true)
    installUpdate.mockResolvedValue({ ok: true, data: install('downloading') })
    fetchUpdateStatus
      .mockResolvedValueOnce({ ok: true, data: status({ install: install('restarting') }) })
      // The service is away while it restarts.
      .mockResolvedValueOnce({ ok: false, status: 0, message: null })
      .mockResolvedValueOnce({ ok: true, data: status({ current_version: '1.8.0', available: null, install: install('done') }) })
    mount()
    await clickInstall()
    expect(installUpdate).toHaveBeenCalledWith(false)
    await vi.advanceTimersByTimeAsync(FOLLOW_INTERVAL_MS)
    expect(screen.getByTestId('update-install-progress').textContent).toContain('Stopping rDownloader')
    await vi.advanceTimersByTimeAsync(FOLLOW_INTERVAL_MS)
    expect(screen.getByTestId('update-install-reconnecting')).toBeTruthy()
    await vi.advanceTimersByTimeAsync(FOLLOW_INTERVAL_MS)
    expect(screen.getByTestId('update-install-outcome').textContent).toContain('Updated to 1.8.0.')
    expect(screen.getByTestId('update-install-reload')).toBeTruthy()
    expect(fetchUpdateStatus).toHaveBeenCalledTimes(3)
  })

  it('shows the old version back with the reason', async () => {
    confirm.mockResolvedValue(true)
    installUpdate.mockResolvedValue({ ok: true, data: install('downloading') })
    fetchUpdateStatus.mockResolvedValue({
      ok: true,
      data: status({ install: install('rolled_back', 'update.health_timeout') })
    })
    mount()
    await clickInstall()
    await vi.advanceTimersByTimeAsync(FOLLOW_INTERVAL_MS)
    const outcome = screen.getByTestId('update-install-outcome').textContent ?? ''
    expect(outcome).toContain('1.8.0 did not start properly, so 1.8.0-beta.2 runs again.')
    expect(outcome).toContain(server.codes['update.health_timeout'])
  })

  it('asks once more when downloads are running, and installs anyway on a yes', async () => {
    confirm.mockResolvedValue(true)
    installUpdate
      .mockResolvedValueOnce({
        ok: false,
        status: 409,
        message: { code: 'update.transfers_active', params: { count: '2' } }
      })
      .mockResolvedValueOnce({ ok: true, data: install('downloading') })
    fetchUpdateStatus.mockResolvedValue({ ok: true, data: status({ install: install('done') }) })
    mount()
    await clickInstall()
    await vi.waitFor(() => expect(installUpdate).toHaveBeenCalledTimes(2))
    expect(confirm).toHaveBeenCalledTimes(2)
    expect(confirm.mock.calls[1]?.[0]?.description).toContain('2 downloads are running')
    expect(installUpdate).toHaveBeenLastCalledWith(true)
  })

  it('says why the update failed and offers to try again', async () => {
    confirm.mockResolvedValue(true)
    installUpdate.mockResolvedValue({ ok: true, data: install('downloading') })
    fetchUpdateStatus.mockResolvedValue({
      ok: true,
      data: status({ install: install('failed', 'update.service_did_not_stop') })
    })
    mount()
    await clickInstall()
    await vi.advanceTimersByTimeAsync(FOLLOW_INTERVAL_MS)
    const outcome = screen.getByTestId('update-install-outcome').textContent ?? ''
    expect(outcome).toContain('The update to 1.8.0 failed.')
    expect(outcome).toContain(server.codes['update.service_did_not_stop'])
    expect(screen.queryByTestId('update-install-reconnecting')).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: system.updates.install.retry }))
    await vi.waitFor(() => expect(installUpdate).toHaveBeenCalledTimes(2))
  })

  it('takes a service that answers as the new version as the update done', async () => {
    confirm.mockResolvedValue(true)
    installUpdate.mockResolvedValue({ ok: true, data: install('downloading') })
    fetchUpdateStatus
      .mockResolvedValueOnce({ ok: false, status: 0, message: null })
      .mockResolvedValueOnce({ ok: true, data: status({ current_version: '1.8.0', available: null, install: null }) })
    mount()
    await clickInstall()
    await vi.advanceTimersByTimeAsync(FOLLOW_INTERVAL_MS * 2)
    expect(screen.getByTestId('update-install-outcome').textContent).toContain('Updated to 1.8.0.')
  })

  it('stops waiting with what to do when the service never comes back', async () => {
    confirm.mockResolvedValue(true)
    installUpdate.mockResolvedValue({ ok: true, data: install('restarting') })
    fetchUpdateStatus.mockResolvedValue({ ok: false, status: 0, message: null })
    mount()
    await clickInstall()
    await vi.advanceTimersByTimeAsync(FOLLOW_LIMIT_MS + FOLLOW_INTERVAL_MS)
    expect(screen.getByTestId('update-install-lost').textContent).toContain(system.updates.install.lost)
    expect(screen.queryByTestId('update-install-reconnecting')).toBeNull()
  })
})

describe('UpdateDetailsModal: downloading ahead of the install', () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    fetchUpdateStatus.mockReset()
    installUpdate.mockReset()
    downloadUpdate.mockReset()
    confirm.mockReset()
    const shared = useUpdateStatus()
    shared.status.value = status()
    shared.installFailure.value = null
    shared.downloadFailure.value = null
    shared.followed.value = false
  })
  afterEach(async () => {
    await settleFollows()
  })

  it('keeps the browser download as a small link beside the background download', () => {
    mount()
    const link = screen.getByTestId('update-download-manual')
    expect(link.getAttribute('to')).toBe(offer.download_url)
    expect(link.textContent).toContain(system.updates.modal.download_manual)
    expect(screen.getByTestId('update-download-start').getAttribute('to')).toBeNull()
  })

  it('offers no browser download for an address that is not http(s) (WEB-1)', () => {
    mount({ ...offer, download_url: 'javascript:alert(1)' })
    expect(screen.getByTestId('update-download-start')).toBeTruthy()
    expect(screen.queryByTestId('update-download-manual')).toBeNull()
  })

  it('downloads in the background with its progress until the file is ready to install', async () => {
    downloadUpdate.mockResolvedValue({ ok: true, data: downloaded('downloading', 0) })
    fetchUpdateStatus
      .mockResolvedValueOnce({ ok: true, data: status({ download: downloaded('downloading', 1024) }) })
      .mockResolvedValueOnce({ ok: true, data: status({ download: downloaded('ready') }) })
    mount()
    await fireEvent.click(screen.getByTestId('update-download-start'))
    await vi.waitFor(() => expect(downloadUpdate).toHaveBeenCalled())
    await vi.advanceTimersByTimeAsync(DOWNLOAD_INTERVAL_MS)
    const progress = screen.getByTestId('update-download-progress').textContent ?? ''
    expect(progress).toContain('Downloading 1.8.0 in the background')
    expect(screen.getByRole('progressbar').getAttribute('aria-label')).toBe('25%')
    await vi.advanceTimersByTimeAsync(DOWNLOAD_INTERVAL_MS)
    expect(screen.getByTestId('update-download-ready').textContent).toContain('1.8.0 is downloaded and verified')
    expect(screen.queryByTestId('update-download-start')).toBeNull()
    expect(screen.getByTestId('update-install-start')).toBeTruthy()
    expect(fetchUpdateStatus).toHaveBeenCalledTimes(2)
  })

  it('says why a download failed', async () => {
    downloadUpdate.mockResolvedValue({ ok: true, data: downloaded('downloading', 0) })
    fetchUpdateStatus.mockResolvedValue({
      ok: true,
      data: status({ download: downloaded('failed', 0, 'update.digest_mismatch') })
    })
    mount()
    await fireEvent.click(screen.getByTestId('update-download-start'))
    await vi.advanceTimersByTimeAsync(DOWNLOAD_INTERVAL_MS)
    const failed = screen.getByTestId('update-download-failed').textContent ?? ''
    expect(failed).toContain('Downloading 1.8.0 failed.')
    expect(failed).toContain(server.codes['update.digest_mismatch'])
    expect(screen.getByTestId('update-download-start')).toBeTruthy()
  })
})

/**
 * What's new (RD-1150-02): the version's points for users as a list, never as markup, and the two
 * links — the full changes at the version's tag, not on `main`, and the release page.
 */
describe('UpdateDetailsModal: what is new', () => {
  beforeEach(() => {
    const shared = useUpdateStatus()
    shared.status.value = status()
    shared.installFailure.value = null
    shared.downloadFailure.value = null
    shared.followed.value = false
  })

  it('lists each point of the notes on its own, as text', () => {
    mount({
      ...offer,
      notes: '- Downloads from file hosters run in parallel.\n\n- A <b>bold</b> claim stays text.'
    })
    const points = screen.getByTestId('update-notes').querySelectorAll('li')
    expect([...points].map(point => point.textContent)).toEqual([
      'Downloads from file hosters run in parallel.',
      'A <b>bold</b> claim stays text.'
    ])
    expect(screen.getByTestId('update-notes').querySelector('b')).toBeNull()
  })

  it('shows a maintenance sentence as its one point', () => {
    mount({ ...offer, notes: 'Maintenance release: internal changes only, no change in behaviour.' })
    expect(screen.getByTestId('update-notes').querySelectorAll('li')).toHaveLength(1)
  })

  it('says so when the release has no notes', () => {
    mount({ ...offer, notes: '\n' })
    expect(screen.queryByTestId('update-notes')).toBeNull()
    expect(screen.getByText(system.updates.modal.no_notes)).toBeTruthy()
  })

  it('links the full changes at the tag and the release page', () => {
    mount()
    const changelog = screen.getByTestId('update-changelog')
    expect(changelog.getAttribute('to')).toBe('https://github.com/degoya/rDownloader/blob/v1.8.0/CHANGELOG.md#180---2026-10-10')
    expect(changelog.textContent).toContain(system.updates.modal.full_changes)
    const release = screen.getByTestId('update-release-page')
    expect(release.getAttribute('to')).toBe('https://github.com/degoya/rDownloader/releases/tag/v1.8.0')
    expect(release.textContent).toContain(system.updates.modal.release_page)
  })

  it('links no address of the offer that is not http(s) (WEB-1)', () => {
    mount({ ...offer, changelog_url: 'javascript:alert(1)', release_url: 'javascript:alert(2)', download_url: 'javascript:alert(3)' })
    expect(screen.queryByTestId('update-changelog')).toBeNull()
    expect(screen.queryByTestId('update-release-page')).toBeNull()
  })
})
