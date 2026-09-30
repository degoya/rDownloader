/**
 * The update details' self-update (RD-180-02): "Install and restart" behind a confirmation, the
 * install followed through a restart that leaves the service silent for a while, the new version
 * or the old one back with its reason, and a refusal for running downloads asked once more.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { UpdateInstall, UpdateOffer, UpdateStatus } from '@/api/updates'
import server from '@/locales/en/server.json'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

const fetchUpdateStatus = vi.fn()
const installUpdate = vi.fn()
const confirm = vi.fn()
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirm }))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))
vi.mock('@/api/updates', async (original) => ({
  ...(await original<typeof import('@/api/updates')>()),
  fetchUpdateStatus: () => fetchUpdateStatus(),
  installUpdate: (allowActive?: boolean) => installUpdate(allowActive)
}))

const { default: UpdateDetailsModal } = await import('./UpdateDetailsModal.vue')
const { useUpdateStatus, FOLLOW_INTERVAL_MS } = await import('@/composables/useUpdateStatus')

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

async function clickInstall(): Promise<void> {
  await fireEvent.click(screen.getByTestId('update-install-start'))
  await vi.waitFor(() => expect(installUpdate).toHaveBeenCalled())
}

describe('UpdateDetailsModal: installing', () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    fetchUpdateStatus.mockReset()
    installUpdate.mockReset()
    confirm.mockReset()
    const shared = useUpdateStatus()
    shared.status.value = status()
    shared.installFailure.value = null
    shared.followed.value = false
  })
  afterEach(() => {
    vi.useRealTimers()
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
})
