/**
 * The offered version at the top of the update card (RD-1150-01): version, date and the first
 * points of the notes, and the actions by how the installation is installed — with the dialog's
 * confirmation and its second question for running downloads.
 */
import { fireEvent, screen, within } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { InstallKind, UpdateInstall, UpdateOffer, UpdateStatus } from '@/api/updates'
import server from '@/locales/en/server.json'
import system from '@/locales/en/system.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

const fetchUpdateStatus = vi.fn()
const installUpdate = vi.fn()
const downloadUpdate = vi.fn()
const confirm = vi.fn()
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirm }))
// The command's copy button reports a refused clipboard with a toast.
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))
vi.mock('@/api/updates', async (original) => ({
  ...(await original<typeof import('@/api/updates')>()),
  fetchUpdateStatus: () => fetchUpdateStatus(),
  installUpdate: (allowActive?: boolean) => installUpdate(allowActive),
  downloadUpdate: () => downloadUpdate()
}))

const { default: UpdateAvailableNotice } = await import('./UpdateAvailableNotice.vue')
const { useUpdateStatus, FOLLOW_INTERVAL_MS } = await import('@/composables/useUpdateStatus')

const offer: UpdateOffer = {
  version: '1.13.0',
  channel: 'stable',
  released_at: '2026-10-05T12:00:00Z',
  notes: 'Added\n- Parallel hoster downloads\n- A diagram for every regex\nFixed\n- The update page names its update\n- A fourth point',
  release_url: 'https://github.com/degoya/rDownloader/releases/tag/v1.13.0',
  changelog_url: 'https://github.com/degoya/rDownloader/blob/v1.13.0/CHANGELOG.md#1130---2026-10-06',
  action: 'install',
  command: null,
  hint: null,
  download_url: 'https://github.com/degoya/rDownloader/releases/download/v1.13.0/rdownloader-linux-x86_64.tar.gz',
  download_size: 4096,
  download_sha256: 'ab'.repeat(32),
  rollback_available: true
}

function install(state: UpdateInstall['state']): UpdateInstall {
  return {
    state,
    from_version: '1.12.0',
    target_version: '1.13.0',
    reason: null,
    started_at: '2026-10-07T13:00:00Z',
    updated_at: '2026-10-07T13:00:05Z'
  }
}

function status(patch: Partial<UpdateStatus> = {}): UpdateStatus {
  return {
    current_version: '1.12.0',
    configured: true,
    check_enabled: true,
    channel: 'stable',
    effective_channel: 'stable',
    interval_hours: 24,
    install_kind: 'portable',
    checking: false,
    last_checked_at: '2026-10-07T13:00:00Z',
    next_check_at: null,
    error_code: null,
    available: offer,
    install: null,
    download: null,
    capture_agents: [],
    ...patch
  }
}

function mount(shown: UpdateOffer = offer, kind: InstallKind = 'portable') {
  return mountComponent(UpdateAvailableNotice, { props: { offer: shown, kind }, messages: { system, server } })
}

describe('UpdateAvailableNotice', () => {
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
    // Lets a follow an install started end, so the next test's own is not taken for it.
    fetchUpdateStatus.mockResolvedValue({ ok: true, data: status({ install: install('done') }) })
    await vi.advanceTimersByTimeAsync(FOLLOW_INTERVAL_MS)
    vi.useRealTimers()
  })

  it('names the version, its date and the first three points of its notes', () => {
    mount()
    const notice = screen.getByTestId('update-available')
    expect(notice.textContent).toContain('Version 1.13.0 is available')
    expect(notice.textContent).toContain('Released:')
    const points = within(screen.getByTestId('update-highlights')).getAllByRole('listitem').map((item) => item.textContent)
    expect(points).toEqual(['Parallel hoster downloads', 'A diagram for every regex', 'The update page names its update'])
  })

  it('offers install, the background download and what is new where the installation installs itself', async () => {
    const { emitted } = mount()
    expect(screen.getByTestId('update-notice-install').textContent).toContain(system.updates.modal.install)
    expect(screen.getByTestId('update-notice-download').textContent).toContain(system.updates.modal.download)
    downloadUpdate.mockResolvedValue({ ok: true, data: { version: '1.13.0', state: 'ready', received_bytes: 4096, total_bytes: 4096, reason: null } })
    await fireEvent.click(screen.getByTestId('update-notice-download'))
    expect(downloadUpdate).toHaveBeenCalledOnce()
    await fireEvent.click(screen.getByRole('button', { name: system.updates.whats_new }))
    expect(emitted().details).toHaveLength(1)
  })

  it('installs only behind the confirmation and opens the dialog to follow it', async () => {
    confirm.mockResolvedValueOnce(false)
    const { emitted } = mount()
    await fireEvent.click(screen.getByTestId('update-notice-install'))
    await vi.waitFor(() => expect(confirm).toHaveBeenCalledOnce())
    expect(installUpdate).not.toHaveBeenCalled()
    expect(emitted().details).toBeUndefined()

    confirm.mockResolvedValueOnce(true)
    installUpdate.mockResolvedValue({ ok: true, data: install('downloading') })
    await fireEvent.click(screen.getByTestId('update-notice-install'))
    await vi.waitFor(() => expect(emitted().details).toHaveLength(1))
    expect(installUpdate).toHaveBeenCalledWith(false)
  })

  it('asks once more while downloads run, as the dialog does', async () => {
    confirm.mockResolvedValue(true)
    installUpdate
      .mockResolvedValueOnce({ ok: false, status: 409, message: { code: 'update.transfers_active', params: { count: '2' } } })
      .mockResolvedValueOnce({ ok: true, data: install('downloading') })
    mount()
    await fireEvent.click(screen.getByTestId('update-notice-install'))
    await vi.waitFor(() => expect(installUpdate).toHaveBeenCalledTimes(2))
    expect(confirm.mock.calls[1]?.[0]?.description).toContain('2 downloads are running')
    expect(installUpdate).toHaveBeenLastCalledWith(true)
  })

  it('says why the service refused the install', async () => {
    confirm.mockResolvedValue(true)
    installUpdate.mockResolvedValue({ ok: false, status: 409, message: { code: 'update.install_running' } })
    const { emitted } = mount()
    await fireEvent.click(screen.getByTestId('update-notice-install'))
    expect((await screen.findByTestId('update-notice-refused')).textContent).toBe(server.codes['update.install_running'])
    expect(emitted().details).toBeUndefined()
  })

  it('links the download for an installation replaced by hand, without an install', () => {
    mount({ ...offer, action: 'download' }, 'unknown')
    expect(screen.queryByTestId('update-notice-install')).toBeNull()
    expect(screen.getByTestId('update-notice-download').getAttribute('to')).toBe(offer.download_url)
  })

  it('shows the package manager\'s command instead of an install', () => {
    mount({ ...offer, action: 'command', command: 'brew upgrade rdownloader', download_url: null }, 'homebrew')
    expect(screen.queryByTestId('update-notice-install')).toBeNull()
    expect(screen.queryByTestId('update-notice-download')).toBeNull()
    const command = screen.getByTestId('update-notice-command')
    expect(command.textContent).toContain('Homebrew manages this installation.')
    expect(within(command).getByDisplayValue('brew upgrade rdownloader')).toBeTruthy()
    expect(screen.getByRole('button', { name: system.updates.whats_new })).toBeTruthy()
  })

  it('renders without an axe violation', async () => {
    const { container } = mount()
    expect(await axeViolations(container)).toBe('')
  })
})
