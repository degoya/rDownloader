/**
 * The tool store was read once on mount and then only from the answer to this card's own
 * actions, so a version installed, activated or rolled back in a second tab — or a manifest the
 * service accepted on its own schedule — left this card offering an "activate" for a version
 * that was already active.
 */
import { screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import settingsCatalogue from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
vi.mock('@/api/client', () => ({
  api: { GET: (...args: unknown[]) => get(...args), POST: vi.fn(async () => ({ data: {} })) },
  responseError: vi.fn(() => 'The service did not answer')
}))

/** The shared event stream, reduced to the one handler this card registers. */
let toolEvent: ((event: MessageEvent<string>) => void) | null = null
const released = vi.fn()
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (handlers: Record<string, (event: MessageEvent<string>) => void>) => {
    toolEvent = handlers['managed_tool.changed'] ?? null
    return () => { toolEvent = null; released() }
  }
}))

vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))

const { default: SettingsManagedTools } = await import('./SettingsManagedTools.vue')

function toolsResponse(activeVersion: string, availableVersion: string) {
  return {
    enabled: true,
    platform: 'linux-x86_64',
    manifest_sequence: 7,
    manifest_url: 'https://tools.example.com/manifest.json',
    tools: [{
      name: 'yt-dlp',
      active_version: activeVersion,
      available_version: availableVersion,
      installed_versions: [activeVersion],
      can_roll_back: false
    }]
  }
}

function activeBadge(version: string): string {
  return settingsCatalogue.managed_tools.active.replace('{version}', version)
}

describe('SettingsManagedTools reacting to managed_tool.changed', () => {
  beforeEach(() => {
    get.mockReset()
    released.mockReset()
    toolEvent = null
  })

  it('re-reads the tool store when a version changes elsewhere', async () => {
    get.mockResolvedValue({ data: toolsResponse('2025.01.15', '2025.09.01') })

    mountComponent(SettingsManagedTools, { messages: { settings: settingsCatalogue } })

    await waitFor(() => expect(screen.getByText(activeBadge('2025.01.15'))).toBeTruthy())

    // The activation happened elsewhere; this card is only told that something changed.
    get.mockResolvedValue({ data: toolsResponse('2025.09.01', '2025.09.01') })
    toolEvent?.({ data: JSON.stringify({ payload: { tool: 'yt-dlp' } }) } as MessageEvent<string>)

    await waitFor(() => expect(screen.getByText(activeBadge('2025.09.01'))).toBeTruthy(), { timeout: 2000 })
  })

  it('coalesces a burst of events into one read', async () => {
    get.mockResolvedValue({ data: toolsResponse('2025.01.15', '2025.09.01') })

    mountComponent(SettingsManagedTools, { messages: { settings: settingsCatalogue } })

    await waitFor(() => expect(screen.getByText(activeBadge('2025.01.15'))).toBeTruthy())
    const before = get.mock.calls.length
    const event = { data: JSON.stringify({ payload: {} }) } as MessageEvent<string>
    for (let index = 0; index < 5; index += 1) toolEvent?.(event)

    // Accepting a manifest touches several tools at once; that must not cost five reads.
    await waitFor(() => expect(get.mock.calls.length).toBe(before + 1), { timeout: 2000 })
    expect(get.mock.calls.length).toBe(before + 1)
  })
})
