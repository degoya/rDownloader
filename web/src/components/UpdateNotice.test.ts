/**
 * The sidebar's update notice (RD-180-01): silent while nothing is offered, one line when a
 * newer version is, and the download for an installation that updates from the archive.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { UpdateOffer, UpdateStatus } from '@/api/updates'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

const fetchUpdateStatus = vi.fn()
// The details' install button asks through Nuxt UI's overlay, which exists only in a Nuxt build.
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))
// The update dialog's copy button reports a refused clipboard with a toast.
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))
vi.mock('@/api/updates', async (original) => ({
  ...(await original<typeof import('@/api/updates')>()),
  fetchUpdateStatus: () => fetchUpdateStatus()
}))

const { default: UpdateNotice } = await import('./UpdateNotice.vue')
const { useUpdateStatus } = await import('@/composables/useUpdateStatus')

const modal = {
  props: ['open', 'title'],
  template: '<div v-if="open" data-testid="modal"><h2>{{ title }}</h2><slot name="body" /><slot name="footer" /></div>'
}

function status(available: UpdateOffer | null): UpdateStatus {
  return {
    current_version: '1.7.0',
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
    available,
    install: null,
    download: null,
    capture_agents: []
  }
}

function mount() {
  return mountComponent(UpdateNotice, {
    messages: { system },
    stubs: { UModal: modal, ULink: { template: '<a v-bind="$attrs"><slot /></a>' } }
  })
}

describe('UpdateNotice', () => {
  beforeEach(() => {
    fetchUpdateStatus.mockReset()
    useUpdateStatus().status.value = null
  })

  it('shows nothing while this installation is up to date', async () => {
    fetchUpdateStatus.mockResolvedValue({ ok: true, data: status(null) })
    mount()
    await vi.waitFor(() => expect(fetchUpdateStatus).toHaveBeenCalled())
    expect(screen.queryByTestId('update-notice')).toBeNull()
  })

  it('names the new version and opens its details with the download', async () => {
    fetchUpdateStatus.mockResolvedValue({
      ok: true,
      data: status({
        version: '1.8.0',
        channel: 'stable',
        released_at: '2026-10-10T12:00:00Z',
        notes: '',
        release_url: 'https://github.com/degoya/rDownloader/releases/tag/v1.8.0',
        action: 'download',
        command: null,
        hint: null,
        download_url: 'https://github.com/degoya/rDownloader/releases/download/v1.8.0/rdownloader-linux-x86_64.tar.gz',
        download_size: 4096,
        download_sha256: 'ab'.repeat(32),
        rollback_available: null
      })
    })
    mount()
    const notice = await screen.findByTestId('update-notice')
    await fireEvent.click(notice.querySelector('button') as HTMLButtonElement)
    expect(screen.getByTestId('update-download').textContent).toContain(system.updates.modal.download_hint)
    expect(screen.getByText(system.updates.modal.no_notes)).toBeTruthy()
    expect(screen.getByRole('button', { name: system.updates.modal.download })).toBeTruthy()
  })
})
