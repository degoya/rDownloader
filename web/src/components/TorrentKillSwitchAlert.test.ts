/**
 * The downloads view's kill-switch warning (RD-190-22): there exactly while the kill switch holds
 * the torrents, naming the interface that went, with the way to the network status.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { TorrentNetworkStatus } from '@/api/types'
import downloads from '@/locales/en/downloads.json'
import { mountComponent } from '@/test/mount'

const get = vi.hoisted(() => vi.fn())
const push = vi.hoisted(() => vi.fn(async () => undefined))
const reveal = vi.hoisted(() => vi.fn(async (..._args: unknown[]) => true))
vi.mock('@/api/client', () => ({ api: { GET: (...args: unknown[]) => get(...args) } }))
vi.mock('vue-router', () => ({ useRouter: () => ({ push }) }))
vi.mock('@/utils/revealAnchor', () => ({ revealAnchor: (...args: unknown[]) => reveal(...args) }))

const { default: TorrentKillSwitchAlert } = await import('./TorrentKillSwitchAlert.vue')

function status(engaged: boolean): TorrentNetworkStatus {
  return {
    bound_interface: 'wg0',
    bound_interface_present: !engaged,
    kill_switch_enabled: true,
    kill_switch_engaged: engaged,
    kill_switch_window_seconds: 10,
    session_generation: 1,
    peer_proxy_configured: false,
    upnp_enabled: false
  }
}

/** The shared stub without the `actions` slot, where the way to the network status sits. */
const UAlert = {
  props: ['title', 'description'],
  template: '<div v-bind="$attrs">{{ title }}{{ description }}<slot name="actions" /></div>'
}

function mount() {
  return mountComponent(TorrentKillSwitchAlert, { messages: { downloads }, stubs: { UAlert } })
}

describe('TorrentKillSwitchAlert', () => {
  afterEach(() => {
    get.mockReset()
    vi.useRealTimers()
  })

  it('stays away while the torrents run', async () => {
    get.mockResolvedValue({ data: status(false) })
    mount()
    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/torrents/network/status'))
    expect(screen.queryByTestId('kill-switch-alert')).toBeNull()
  })

  it('names the interface that went and leads to the network status', async () => {
    get.mockResolvedValue({ data: status(true) })
    mount()
    expect(await screen.findByText(downloads.kill_switch.title, { exact: false })).not.toBeNull()
    expect(screen.getByTestId('kill-switch-alert').textContent).toContain('wg0')

    await fireEvent.click(screen.getByRole('button', { name: downloads.kill_switch.open }))
    expect(push).toHaveBeenCalledWith('/settings/torrent')
    await waitFor(() => expect(reveal).toHaveBeenCalledWith('torrent.network_status', { focus: false }))
  })

  it('goes away on its own once the interface is back', async () => {
    vi.useFakeTimers()
    get.mockResolvedValueOnce({ data: status(true) }).mockResolvedValue({ data: status(false) })
    mount()
    await vi.waitFor(() => expect(screen.queryByTestId('kill-switch-alert')).not.toBeNull())

    await vi.advanceTimersByTimeAsync(10_000)
    await vi.waitFor(() => expect(screen.queryByTestId('kill-switch-alert')).toBeNull())
  })
})
