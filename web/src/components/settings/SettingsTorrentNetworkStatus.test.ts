/**
 * The torrent engine's network status in the torrent settings (RD-190-22): the bound interface
 * and whether it is there, the kill switch's state, and a failed session rebuild.
 */
import { screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { TorrentNetworkStatus } from '@/api/types'
import settings from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

const get = vi.hoisted(() => vi.fn())
vi.mock('@/api/client', () => ({
  api: { GET: (...args: unknown[]) => get(...args) },
  responseError: () => 'The service did not answer'
}))

const { default: SettingsTorrentNetworkStatus } = await import('./SettingsTorrentNetworkStatus.vue')

const BASE: TorrentNetworkStatus = {
  bound_interface: null,
  bound_interface_present: false,
  kill_switch_enabled: false,
  kill_switch_engaged: false,
  kill_switch_window_seconds: 10,
  session_generation: 1,
  peer_proxy_configured: false,
  upnp_enabled: false
}

function mount(status: Partial<TorrentNetworkStatus>) {
  get.mockResolvedValue({ data: { ...BASE, ...status } })
  return mountComponent(SettingsTorrentNetworkStatus, { messages: { settings } })
}

const s = settings.torrent.network_status

describe('SettingsTorrentNetworkStatus', () => {
  it('says all interfaces and an idle kill switch when nothing is bound', async () => {
    mount({})
    expect(await screen.findByText(settings.torrent.bind_interface.any)).not.toBeNull()
    expect(screen.getByTestId('kill-switch-state').textContent?.trim()).toBe(s.kill_switch_off)
  })

  it('shows an armed kill switch with its window while the interface is there', async () => {
    mount({ bound_interface: 'wg0', bound_interface_present: true, kill_switch_enabled: true })
    expect(await screen.findByText(s.interface_present)).not.toBeNull()
    expect(screen.getByTestId('kill-switch-state').textContent).toContain('10 s')
    expect(screen.queryByText(s.engaged_title)).toBeNull()
  })

  it('warns that the kill switch holds the torrents while the interface is gone', async () => {
    mount({ bound_interface: 'wg0', kill_switch_enabled: true, kill_switch_engaged: true })
    expect(await screen.findByText(s.engaged_title, { exact: false })).not.toBeNull()
    expect(screen.getByText(s.interface_missing)).not.toBeNull()
    expect(screen.getByTestId('kill-switch-state').textContent?.trim()).toBe(s.kill_switch_engaged)
  })

  it('names a failed session rebuild and an unresolvable peer proxy with their reasons', async () => {
    mount({ last_rebuild_error: 'listen port 6881 is taken', peer_proxy_configured: true, peer_proxy_error: 'proxy profile not found' })
    expect(await screen.findByText(s.rebuild_failed, { exact: false })).not.toBeNull()
    expect(screen.getByText('listen port 6881 is taken', { exact: false })).not.toBeNull()
    expect(screen.getByText(s.proxy_failed, { exact: false })).not.toBeNull()
    expect(screen.getByText(s.peer_proxy_on)).not.toBeNull()
  })
})
