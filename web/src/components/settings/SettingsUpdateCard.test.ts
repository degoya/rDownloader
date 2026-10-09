/**
 * The update card of Settings > System (RD-180-01): what runs and what is offered, "check now",
 * the refusal a check reports, the build without an update key, and the channel a package
 * manager cannot follow. The offered version at the top of the card, and an ended update's
 * outcome only while it fits the running version (RD-1150-01).
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { UpdateInstall, UpdateStatus } from '@/api/updates'
import server from '@/locales/en/server.json'
import system from '@/locales/en/system.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

const fetchUpdateStatus = vi.fn()
const checkForUpdates = vi.fn()
// The details' install button asks through Nuxt UI's overlay, which exists only in a Nuxt build.
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))
// The update dialog's copy button reports a refused clipboard with a toast.
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))
vi.mock('@/api/updates', async (original) => ({
  ...(await original<typeof import('@/api/updates')>()),
  fetchUpdateStatus: () => fetchUpdateStatus(),
  checkForUpdates: () => checkForUpdates()
}))

const { default: SettingsUpdateCard } = await import('./SettingsUpdateCard.vue')
const { useUpdateStatus } = await import('@/composables/useUpdateStatus')

const modal = {
  props: ['open', 'title', 'description'],
  template: '<div v-if="open" data-testid="modal"><h2>{{ title }}</h2><slot name="body" /><slot name="footer" /></div>'
}

function status(patch: Partial<UpdateStatus> = {}): UpdateStatus {
  return {
    current_version: '1.8.0-beta.1',
    configured: true,
    check_enabled: true,
    channel: 'stable',
    effective_channel: 'stable',
    interval_hours: 24,
    install_kind: 'portable',
    checking: false,
    last_checked_at: null,
    next_check_at: null,
    error_code: null,
    available: null,
    install: null,
    download: null,
    capture_agents: [],
    ...patch
  }
}

const offer = {
  version: '1.8.0',
  channel: 'stable' as const,
  released_at: '2026-10-10T12:00:00Z',
  notes: 'Added\n- Update check (RD-180-01).',
  release_url: 'https://github.com/degoya/rDownloader/releases/tag/v1.8.0',
  changelog_url: 'https://github.com/degoya/rDownloader/blob/v1.8.0/CHANGELOG.md#180---2026-10-10',
  action: 'command' as const,
  command: 'brew upgrade rdownloader',
  hint: null,
  download_url: null,
  download_size: null,
  download_sha256: null,
  rollback_available: null
}

function mount(settings: Record<string, unknown> = {}) {
  return mountComponent(SettingsUpdateCard, {
    props: {
      modelValue: { update_check_enabled: true, update_channel: 'stable', update_check_interval_hours: 24, ...settings }
    },
    messages: { system, server },
    stubs: { UModal: modal, ULink: { template: '<a v-bind="$attrs"><slot /></a>' } }
  })
}

describe('SettingsUpdateCard', () => {
  beforeEach(() => {
    fetchUpdateStatus.mockReset()
    checkForUpdates.mockReset()
    useUpdateStatus().status.value = null
  })

  it('shows the running version, how it was installed, and that nothing was checked yet', async () => {
    fetchUpdateStatus.mockResolvedValue({ ok: true, data: status() })
    mount()
    const block = await screen.findByTestId('update-status')
    expect(block.textContent).toContain('Installed: 1.8.0-beta.1')
    expect(block.textContent).toContain('Installed as: Portable archive')
    expect(block.textContent).toContain(system.updates.never_checked)
    expect(screen.queryByTestId('update-available')).toBeNull()
  })

  it('checks on a click and offers the new version with its command', async () => {
    fetchUpdateStatus.mockResolvedValue({ ok: true, data: status() })
    checkForUpdates.mockResolvedValue({
      ok: true,
      data: status({ install_kind: 'homebrew', last_checked_at: '2026-10-10T13:00:00Z', available: offer })
    })
    mount()
    await screen.findByTestId('update-status')
    await fireEvent.click(screen.getByTestId('update-check-now'))
    expect(checkForUpdates).toHaveBeenCalledOnce()
    const available = await screen.findByTestId('update-available')
    expect(available.textContent).toContain('Version 1.8.0 is available')
    expect(screen.queryByTestId('update-current')).toBeNull()

    await fireEvent.click(screen.getByRole('button', { name: system.updates.whats_new }))
    const details = await screen.findByTestId('update-details')
    expect(details.textContent).toContain('Update check (RD-180-01).')
    expect(within(screen.getByTestId('update-command')).getByDisplayValue('brew upgrade rdownloader')).toBeTruthy()
    expect(screen.getByTestId('update-command').textContent).toContain('Homebrew manages this installation.')
  })

  it('says in words why the last check refused the manifest', async () => {
    fetchUpdateStatus.mockResolvedValue({
      ok: true,
      data: status({ last_checked_at: '2026-10-10T13:00:00Z', error_code: 'update.bad_signature' })
    })
    mount()
    const error = await screen.findByTestId('update-error')
    expect(error.textContent).toBe(server.codes['update.bad_signature'])
    expect(screen.queryByTestId('update-current')).toBeNull()
  })

  it('reports up to date after a clean check', async () => {
    fetchUpdateStatus.mockResolvedValue({ ok: true, data: status({ last_checked_at: '2026-10-10T13:00:00Z' }) })
    mount()
    expect((await screen.findByTestId('update-current')).textContent).toContain(system.updates.up_to_date)
  })

  it('switches "check now" off in a build without the update key and says why', async () => {
    fetchUpdateStatus.mockResolvedValue({ ok: true, data: status({ configured: false }) })
    mount()
    await screen.findByText(system.updates.not_configured)
    await waitFor(() => expect(screen.getByTestId('update-check-now').hasAttribute('disabled')).toBe(true))
  })

  it('tells a Homebrew installation that it stays on stable when beta is chosen', async () => {
    fetchUpdateStatus.mockResolvedValue({ ok: true, data: status({ install_kind: 'homebrew' }) })
    mount({ update_channel: 'beta' })
    expect((await screen.findByTestId('update-channel-forced')).textContent).toContain(system.updates.channel_forced_stable)
  })

  it('shows the failure of a check that could not run', async () => {
    fetchUpdateStatus.mockResolvedValue({ ok: true, data: status() })
    checkForUpdates.mockResolvedValue({ ok: false, status: 409, message: { code: 'update.not_configured' } })
    mount()
    await screen.findByTestId('update-status')
    await fireEvent.click(screen.getByTestId('update-check-now'))
    expect((await screen.findByTestId('update-error')).textContent).toBe(server.codes['update.not_configured'])
  })
  it('names a running capture agent of the same version and says nothing more', async () => {
    fetchUpdateStatus.mockResolvedValue({
      ok: true,
      data: status({ capture_agents: [{ version: '1.8.0-beta.1', outdated: false }] })
    })
    mount()
    expect((await screen.findByTestId('update-capture-agents')).textContent).toContain('Capture agent: 1.8.0-beta.1')
    expect(screen.queryByTestId('update-capture-outdated')).toBeNull()
  })

  it('tells how to restart a capture agent older than the service, with both versions', async () => {
    fetchUpdateStatus.mockResolvedValue({
      ok: true,
      data: status({
        capture_agents: [
          { version: null, outdated: true },
          { version: '1.8.0-beta.1', outdated: false }
        ]
      })
    })
    mount()
    expect((await screen.findByTestId('update-capture-agents')).textContent)
      .toContain('Capture agent: before 1.9, 1.8.0-beta.1')
    const hint = screen.getByTestId('update-capture-outdated')
    expect(hint.textContent).toContain(system.updates.agents.outdated_title)
    expect(hint.textContent).toContain('still runs version before 1.9, rDownloader already runs 1.8.0-beta.1')
  })

  it('names the update an agent installed without the service offers itself, without a restart hint', async () => {
    fetchUpdateStatus.mockResolvedValue({
      ok: true,
      data: status({
        capture_agents: [
          { version: '1.7.0', outdated: true, self_update: 'offered', offered_version: '1.8.0', remote_update_allowed: false }
        ]
      })
    })
    mount()
    expect((await screen.findByTestId('update-capture-self-offered')).textContent)
      .toContain('offers itself version 1.8.0')
    expect(screen.queryByTestId('update-capture-outdated')).toBeNull()
  })

  it('has no word about capture agents when none runs', async () => {
    fetchUpdateStatus.mockResolvedValue({ ok: true, data: status() })
    mount()
    await screen.findByTestId('update-status')
    expect(screen.queryByTestId('update-capture-agents')).toBeNull()
    expect(screen.queryByTestId('update-capture-outdated')).toBeNull()
  })

  it('keeps quiet without an update: no notice, only that the version is the newest', async () => {
    fetchUpdateStatus.mockResolvedValue({ ok: true, data: status({ last_checked_at: '2026-10-10T13:00:00Z' }) })
    const { container } = mount()
    await screen.findByTestId('update-current')
    expect(screen.queryByTestId('update-available')).toBeNull()
    expect(await axeViolations(container)).toBe('')
  })

  it('puts the offered version at the top of the card with its actions', async () => {
    fetchUpdateStatus.mockResolvedValue({
      ok: true,
      data: status({ last_checked_at: '2026-10-10T13:00:00Z', available: { ...offer, action: 'install', command: null } })
    })
    const { container } = mount()
    const available = await screen.findByTestId('update-available')
    // Above the status block, not a badge inside it.
    expect(available.compareDocumentPosition(screen.getByTestId('update-status')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(within(available).getByRole('button', { name: system.updates.modal.install })).toBeTruthy()
    expect(within(available).getByRole('button', { name: system.updates.whats_new })).toBeTruthy()
    expect(await axeViolations(container)).toBe('')
  })

  const ended = (state: UpdateInstall['state'], from: string, target: string): UpdateInstall => ({
    state, from_version: from, target_version: target, reason: null,
    started_at: '2026-10-01T10:00:00Z', updated_at: '2026-10-01T10:02:00Z'
  })

  it('no longer says "Updated to 1.11.0" once 1.12.0 runs', async () => {
    fetchUpdateStatus.mockResolvedValue({
      ok: true,
      data: status({ current_version: '1.12.0', install: ended('done', '1.10.0', '1.11.0') })
    })
    mount()
    await screen.findByTestId('update-status')
    expect(screen.queryByTestId('update-install-last')).toBeNull()
  })

  it('says "Updated to 1.12.0" while 1.12.0 runs', async () => {
    fetchUpdateStatus.mockResolvedValue({
      ok: true,
      data: status({ current_version: '1.12.0', install: ended('done', '1.11.0', '1.12.0') })
    })
    mount()
    expect((await screen.findByTestId('update-install-last')).textContent).toContain('Updated to 1.12.0.')
  })

  it('keeps a rollback while the version it went back to runs, and drops it after a later update', async () => {
    fetchUpdateStatus.mockResolvedValue({
      ok: true,
      data: status({ current_version: '1.11.0', install: ended('rolled_back', '1.11.0', '1.12.0') })
    })
    mount()
    expect((await screen.findByTestId('update-install-last')).textContent).toContain('1.11.0 runs again')
    fetchUpdateStatus.mockResolvedValue({
      ok: true,
      data: status({ current_version: '1.13.0', install: ended('rolled_back', '1.11.0', '1.12.0') })
    })
    await useUpdateStatus().load()
    await waitFor(() => expect(screen.queryByTestId('update-install-last')).toBeNull())
  })
})
