import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { Settings } from '@/api/types'
import en from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

import SettingsTorrentCard from './SettingsTorrentCard.vue'

const post = vi.hoisted(() => vi.fn())
vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: undefined })), POST: post },
  responseError: vi.fn(() => 'refused')
}))
// The shared lists follow the event stream (WEB-3); jsdom has no `EventSource`.
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))

/**
 * The shared field stub, plus the description: the leech-only warning lives in that slot. The
 * label wraps the control and names it, as `for` does live.
 */
const UFormField = {
  props: ['label', 'description'],
  template:
    '<div v-bind="$attrs"><label v-if="label">{{ label }}<slot /></label><slot v-else />'
    + '<p><slot name="description">{{ description }}</slot></p></div>'
}

function mount(settings: Partial<Settings>) {
  return mountComponent(SettingsTorrentCard, {
    messages: { settings: en },
    props: { modelValue: settings as Settings },
    stubs: { UFormField }
  })
}

describe('SettingsTorrentCard sharing', () => {
  it('keeps seeding out of reach while nothing is shared', () => {
    // Seeding is a form of sharing. Leaving its switch usable while sharing is off would
    // promise an upload that the engine refuses to make.
    mount({ torrent_sharing_enabled: false, torrent_seeding_enabled: false })

    expect(screen.getByTestId('torrent-sharing').getAttribute('aria-checked')).toBe('false')
    expect(screen.getByRole('switch', { name: en.torrent.seeding.label }).hasAttribute('disabled')).toBe(true)
    expect(screen.queryByText(en.torrent.sharing.leech_only_warning)).not.toBeNull()
  })

  it('releases seeding once sharing is on', () => {
    mount({ torrent_sharing_enabled: true, torrent_seeding_enabled: false })

    expect(screen.getByRole('switch', { name: en.torrent.seeding.label }).hasAttribute('disabled')).toBe(false)
    expect(screen.queryByText(en.torrent.sharing.leech_only_warning)).toBeNull()
  })
})

/**
 * RD-150-11: a switch stands directly before the fields it governs. Sharing and seeding sat at
 * the top of the card while the upload limit, ratio and time they lock were fourteen fields
 * further down, so switching one off greyed out something the reader could not see.
 */
describe('SettingsTorrentCard order', () => {
  function labels(): string[] {
    return Array.from(document.querySelectorAll('label')).map(label => label.textContent?.trim() ?? '')
  }

  it('puts each governed field right after the switch that governs it', () => {
    mount({ torrent_sharing_enabled: true, torrent_seeding_enabled: true })
    const order = labels()
    const at = (label: string) => order.indexOf(label)

    expect(at(en.torrent.sharing.label)).toBe(0)
    expect(at(en.torrent.upload_limit.label)).toBe(at(en.torrent.sharing.label) + 1)
    expect(at(en.torrent.seeding.label)).toBe(at(en.torrent.upload_limit.label) + 1)
    expect(at(en.torrent.seed_ratio.label)).toBe(at(en.torrent.seeding.label) + 1)
    expect(at(en.torrent.seed_time.label)).toBe(at(en.torrent.seed_ratio.label) + 1)
  })
})

/** RD-1240-16: the active limits and the port test, which asks this machine alone. */
describe('SettingsTorrentCard active limits and port test', () => {
  it('ties the seed limit to seeding and keeps the download limit free', () => {
    mount({ torrent_sharing_enabled: true, torrent_seeding_enabled: false, torrent_max_active_downloads: 4 })

    expect(screen.getByRole('spinbutton', { name: en.torrent.max_active_seeds.label }).hasAttribute('disabled')).toBe(true)
    expect(screen.getByRole('spinbutton', { name: en.torrent.max_active_downloads.label }).hasAttribute('disabled')).toBe(false)
  })

  it('shows a listening port as unproven, with what the test saw', async () => {
    post.mockResolvedValueOnce({
      data: {
        verdict: 'listening',
        listen_port: 51413,
        announce_port: 51413,
        local_tcp_accepted: true,
        live_torrents: 2,
        incoming_peers: 0,
        upnp_enabled: false,
        peer_proxy_configured: false,
        error: null,
        tested_at: '2026-10-10T12:00:00Z'
      }
    })
    mount({ torrent_sharing_enabled: true })

    await fireEvent.click(screen.getByTestId('torrent-port-test'))

    expect(post).toHaveBeenCalledWith('/api/v1/torrents/network/port-test')
    await waitFor(() => expect(screen.getByText(en.torrent.port_test.verdict.listening.replace('{port}', '51413'))).toBeTruthy())
    expect(screen.getByText('Torrents running: 2 · peers connected in: 0')).toBeTruthy()
  })

  it('shows the refusal when the test cannot run', async () => {
    post.mockResolvedValueOnce({ data: undefined, error: { code: 'torrent.service_disabled' } })
    mount({ torrent_sharing_enabled: true })

    await fireEvent.click(screen.getByTestId('torrent-port-test'))

    await waitFor(() => expect(screen.getByText('refused')).toBeTruthy())
    expect(screen.queryByTestId('torrent-port-test-result')).toBeNull()
  })
})
