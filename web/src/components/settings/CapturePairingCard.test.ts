/**
 * Pairing a capture agent with or without queue control (RD-1100-06): the right to pause the
 * queue from the tray is asked for explicitly, and only the desktop agent is offered it.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

import CapturePairingCard from './CapturePairingCard.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'refused')
}))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))

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

  it('pairs the desktop agent without queue control unless it is ticked', async () => {
    const { container } = renderCard()
    expect(await pair(container)).toEqual({ body: { label: 'Windows 11', queue_control: false } })
  })

  it('asks for queue control when the box is ticked', async () => {
    const { container } = renderCard()
    await fireEvent.click(screen.getByRole('checkbox', { name: system.pairing.queue_control }))
    expect(await pair(container)).toEqual({ body: { label: 'Windows 11', queue_control: true } })
  })

  it('offers the browser extension no queue control', async () => {
    const { container } = renderCard(true)
    expect(screen.queryByRole('checkbox', { name: system.pairing.queue_control })).toBeNull()
    const sent = await pair(container) as { body: { queue_control: boolean } }
    expect(sent.body.queue_control).toBe(false)
  })
})
