/**
 * Pairing a capture agent with or without queue control (RD-1100-06) and the server update
 * (RD-1240-25): each right is asked for explicitly, and only the desktop agent is offered them.
 * After pairing, the card names the server address and the tray's link route (RD-1390-05).
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock, confirmMock, toastMock } from '@/test/mocks'

import { api } from '@/api/client'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

import CapturePairingCard from './CapturePairingCard.vue'

vi.mock('@/api/client', () => apiClientMock({ responseError: vi.fn(() => 'refused') }))
vi.mock('@nuxt/ui/composables', () => toastMock())
vi.mock('@/composables/useConfirm', () => confirmMock())

const TOKEN = {
  id: 'agent-1',
  label: 'Windows 11',
  scopes: ['capture:*'],
  created_at: '2026-10-04T00:00:00Z',
  last_used_at: null,
  revoked_at: null
}

function renderCard(extension = false) {
  return mountComponent(CapturePairingCard, {
    props: { modelValue: [], extension },
    messages: { system }
  })
}

async function pair(container: Element): Promise<unknown> {
  const form = container.querySelector('form')
  if (!form) throw new Error('the pairing form is missing')
  await fireEvent.submit(form)
  return vi.mocked(api.POST).mock.calls[0]?.[1]
}

describe('CapturePairingCard', () => {
  beforeEach(() => {
    vi.mocked(api.POST).mockReset()
    vi.mocked(api.POST).mockResolvedValue({ data: { bearer: 'secret', token: TOKEN } } as never)
  })

  it('pairs the desktop agent without queue control or server update unless ticked', async () => {
    const { container } = renderCard()
    expect(await pair(container)).toEqual({ body: { label: 'Windows 11', queue_control: false, server_update: false, expires_in_days: null } })
  })

  it('asks for queue control when the box is ticked', async () => {
    const { container } = renderCard()
    await fireEvent.click(screen.getByRole('checkbox', { name: system.pairing.queue_control }))
    expect(await pair(container)).toEqual({ body: { label: 'Windows 11', queue_control: true, server_update: false, expires_in_days: null } })
  })

  it('asks for the server update right when its box is ticked', async () => {
    const { container } = renderCard()
    await fireEvent.click(screen.getByRole('checkbox', { name: system.pairing.server_update }))
    expect(await pair(container)).toEqual({ body: { label: 'Windows 11', queue_control: false, server_update: true, expires_in_days: null } })
  })

  it('offers the browser extension no queue control and no server update', async () => {
    const { container } = renderCard(true)
    expect(screen.queryByRole('checkbox', { name: system.pairing.queue_control })).toBeNull()
    expect(screen.queryByRole('checkbox', { name: system.pairing.server_update })).toBeNull()
    const sent = await pair(container) as { body: { queue_control: boolean, server_update: boolean } }
    expect(sent.body.queue_control).toBe(false)
    expect(sent.body.server_update).toBe(false)
  })

  it('shows the server address to copy', async () => {
    const { container } = renderCard()
    await pair(container)
    const field = await screen.findByRole('textbox', { name: system.pairing.copy_service })
    expect((field as HTMLInputElement).value).toBe(window.location.origin)
  })

  it('names the tray route', async () => {
    const { container } = renderCard()
    expect(screen.queryByText(system.pairing.tray_hint)).toBeNull()
    await pair(container)
    await waitFor(() => expect(screen.getByText(system.pairing.tray_hint)).toBeTruthy())
  })

  it('warns that a loopback address works on this computer only', async () => {
    // The test document is served from localhost, the case the hint is for.
    expect(window.location.hostname).toBe('localhost')
    const { container } = renderCard()
    await pair(container)
    expect(await screen.findByTestId('pairing-loopback-hint')).toBeTruthy()
  })
})
