/**
 * API tokens, checked on the four things that decide what a minted token can do.
 *
 * This card mints bearer tokens for the MCP endpoint, so the failures worth a test are the ones
 * that hand out more reach than the reader meant to, or that make a correct token look broken.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import en from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

import SettingsMcpAccess from './SettingsMcpAccess.vue'

const AREAS = [
  { scope: 'api:read', sensitive: false, operations: 40, implies: [] },
  { scope: 'api:settings', sensitive: true, operations: 12, implies: ['api:read'] }
]

const post = vi.fn(async () => ({
  data: { bearer: 'rdp_secret', token: { id: 't1', label: 'Claude', scopes: ['api:read'], created_at: '2026-09-18T10:00:00Z' } }
}))

vi.mock('@/api/client', () => ({
  api: {
    GET: vi.fn(async (path: string) => ({ data: path.endsWith('/scopes') ? AREAS : [] })),
    POST: (...args: unknown[]) => post(...(args as [])),
    DELETE: vi.fn(async () => ({ data: { message: 'gone' } }))
  },
  responseError: () => 'failed'
}))

vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => async () => true }))
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(true) }) }) })
}))

function mount() {
  return mountComponent(SettingsMcpAccess, { messages: { system: en } })
}

describe('SettingsMcpAccess', () => {
  /** A form that opens on "everything" is a form whose default everybody keeps. */
  it('opens on the reading area alone, not on everything', async () => {
    mount()
    await waitFor(() => expect(screen.getAllByRole('checkbox')).toHaveLength(2))
    const [read, settings] = screen.getAllByRole('checkbox') as HTMLInputElement[]
    expect(read?.checked).toBe(true)
    expect(settings?.checked).toBe(false)
  })

  it('warns only once a sensitive area is actually chosen', async () => {
    mount()
    await waitFor(() => expect(screen.getAllByRole('checkbox')).toHaveLength(2))
    expect(screen.queryByText(en.mcp.sensitive_warning)).toBeNull()

    await fireEvent.click(screen.getAllByRole('checkbox')[1] as HTMLElement)
    await waitFor(() => expect(screen.getByText(en.mcp.sensitive_warning)).toBeTruthy())
  })

  /**
   * Clients that read the header out of an environment variable want `Bearer <token>` in it.
   * Offering the raw token invites the failure that is indistinguishable from a broken server.
   */
  it('offers the header with its Bearer prefix, not the bare token', async () => {
    mount()
    await waitFor(() => expect(screen.getAllByRole('checkbox')).toHaveLength(2))
    await fireEvent.submit(document.querySelector('form') as HTMLFormElement)
    await waitFor(() => expect(screen.getByText('rdp_secret')).toBeTruthy())
    expect(screen.getByText('Bearer rdp_secret')).toBeTruthy()
  })

  it('shows no token at all before one has been minted', async () => {
    mount()
    await waitFor(() => expect(screen.getAllByRole('checkbox')).toHaveLength(2))
    expect(screen.queryByText(en.mcp.copy_hint)).toBeNull()
  })
})
