/**
 * API tokens, checked on the four things that decide what a minted token can do.
 *
 * This card mints bearer tokens for the MCP endpoint, so the failures worth a test are the ones
 * that hand out more reach than the reader meant to, or that make a correct token look broken.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import common from '@/locales/en/common.json'
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

/**
 * The areas were a column of hand-built label rows under a `<p>`; they are one `UCheckboxGroup`
 * now, so the group carries its own name and each area its checked state (RD-150-11).
 */
describe('SettingsMcpAccess areas as a group', () => {
  it('names the areas as one group, each area a checkbox named by the area', async () => {
    mount()
    const group = await screen.findByRole('group', { name: en.mcp.scopes_label })
    await waitFor(() => expect(within(group).getAllByRole('checkbox')).toHaveLength(2))
    expect(within(group).getByRole('checkbox', { name: en.mcp.areas.read.name })).toBeTruthy()
  })

  it('ends the form with the create action, after the areas it depends on', async () => {
    mount()
    const group = await screen.findByRole('group', { name: en.mcp.scopes_label })
    const create = screen.getByRole('button', { name: en.mcp.submit })
    expect(group.compareDocumentPosition(create) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(create.getAttribute('type')).toBe('submit')
  })

  it('edits a token\'s areas in a group of its own and leaves with the cross', async () => {
    const { api } = await import('@/api/client')
    vi.mocked(api.GET).mockImplementation(async (path: string) => ({
      data: path.endsWith('/scopes')
        ? AREAS
        : [{ id: 't1', label: 'Claude', scopes: ['api:read'], created_at: '2026-09-18T10:00:00Z' }]
    }) as never)
    mount()
    await fireEvent.click(await screen.findByRole('button', { name: en.mcp.edit.label }))
    expect(screen.getAllByRole('group', { name: en.mcp.scopes_label })).toHaveLength(2)
    await fireEvent.click(screen.getByRole('button', { name: common.actions.cancel_edit }))
    expect(screen.getAllByRole('group', { name: en.mcp.scopes_label })).toHaveLength(1)
  })
})
