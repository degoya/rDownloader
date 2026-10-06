import { screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

import SettingsReadinessCard from './SettingsReadinessCard.vue'

vi.mock('@/api/client', () => ({ api: { GET: vi.fn() } }))
vi.mock('@/stores/session', () => ({
  useSessionStore: () => ({ setupRequired: false, loginDisabled: false })
}))

function mount() {
  return mountComponent(SettingsReadinessCard, { messages: { system } })
}

function status(overrides: Record<string, unknown> = {}) {
  return {
    data: {
      wizard_completed: true,
      storage_roots: 1,
      ephemeral_storage_roots: 0,
      categories: 1,
      capture_agents: 0,
      accounts: 0,
      usenet_servers: 0,
      ...overrides
    }
  }
}

describe('SettingsReadinessCard', () => {
  beforeEach(() => {
    vi.mocked(api.GET).mockReset()
  })

  it('reports a storage root that will not survive without calling the setup unfinished', async () => {
    vi.mocked(api.GET).mockResolvedValue(status({ ephemeral_storage_roots: 1 }) as never)

    mount()

    await waitFor(() => {
      expect(screen.getByText(system.readiness.checks.storage.ephemeral)).toBeTruthy()
    })
    // Misconfigured is not the same as missing: a complete setup must not be reported as
    // having outstanding steps just because one root sits in the wrong place.
    expect(screen.getByText(system.readiness.complete)).toBeTruthy()
  })

  it('says the storage step is done when every root is persistent', async () => {
    vi.mocked(api.GET).mockResolvedValue(status() as never)

    mount()

    await waitFor(() => {
      expect(screen.getByText(system.readiness.checks.storage.done)).toBeTruthy()
    })
    expect(screen.queryByText(system.readiness.checks.storage.ephemeral)).toBeNull()
  })

  it('still reports a missing storage root as missing', async () => {
    vi.mocked(api.GET).mockResolvedValue(status({ storage_roots: 0, wizard_completed: false }) as never)

    mount()

    await waitFor(() => {
      expect(screen.getByText(system.readiness.checks.storage.missing)).toBeTruthy()
    })
  })

  it('lays the checks out in two columns from the medium breakpoint', async () => {
    vi.mocked(api.GET).mockResolvedValue(status() as never)

    const { container } = mount()

    await waitFor(() => expect(container.querySelector('ul')).toBeTruthy())
    expect(container.querySelector('ul')?.className).toContain('md:grid-cols-2')
  })
})
