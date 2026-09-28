/**
 * RD-150-01: a download waiting for a collision answer is shown with the three answers, and an
 * answer is sent for that download alone.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import downloads from '@/locales/en/downloads.json'
import server from '@/locales/en/server.json'
import { mountComponent } from '@/test/mount'

const list = vi.fn()
const decide = vi.fn()
vi.mock('@/api/storage', async (original) => ({
  ...(await original<typeof import('@/api/storage')>()),
  listCollisionPrompts: () => list(),
  decideCollision: (id: string, decision: string) => decide(id, decision)
}))

const { default: CollisionPromptsAlert } = await import('./CollisionPromptsAlert.vue')

/** The alert with its named slots, where the description and the answers live. */
const stubs = {
  UAlert: {
    props: ['title'],
    template: '<div v-bind="$attrs">{{ title }}<slot name="description" /><slot name="actions" /></div>'
  }
}

const PROMPT = {
  download_id: 'download-1',
  package_id: 'package-1',
  package_name: 'Release',
  target_name: 'file.bin',
  phase: 'before_transfer',
  existing_bytes: 2048,
  decision: null,
  created_at: '2026-09-27T10:00:00Z',
  decided_at: null
}

describe('CollisionPromptsAlert', () => {
  beforeEach(() => {
    list.mockReset()
    decide.mockReset()
  })

  it('shows nothing while no download waits', async () => {
    list.mockResolvedValue({ ok: true, data: [] })
    const { container } = mountComponent(CollisionPromptsAlert, { messages: { downloads, server }, stubs })
    await waitFor(() => expect(list).toHaveBeenCalled())
    expect(container.querySelector('[data-testid="collision-prompts"]')).toBeNull()
  })

  it('offers the three answers and sends the chosen one for that download', async () => {
    list.mockResolvedValueOnce({ ok: true, data: [PROMPT] }).mockResolvedValue({ ok: true, data: [] })
    decide.mockResolvedValue({ ok: true, data: { code: 'collision.decided', message: '' } })
    mountComponent(CollisionPromptsAlert, { messages: { downloads, server }, stubs })

    expect(await screen.findByText(downloads.collision.prompts.title.replace('{name}', 'file.bin'))).toBeTruthy()
    for (const label of Object.values(downloads.collision.decisions)) {
      expect(screen.getByRole('button', { name: label })).toBeTruthy()
    }
    await fireEvent.click(screen.getByRole('button', { name: downloads.collision.decisions.overwrite }))
    await waitFor(() => expect(decide).toHaveBeenCalledWith('download-1', 'overwrite'))
    await waitFor(() => expect(screen.queryByText(downloads.collision.prompts.title.replace('{name}', 'file.bin'))).toBeNull())
  })

  it('hides a prompt that already holds an answer', async () => {
    list.mockResolvedValue({ ok: true, data: [{ ...PROMPT, decision: 'rename' }] })
    const { container } = mountComponent(CollisionPromptsAlert, { messages: { downloads, server }, stubs })
    await waitFor(() => expect(list).toHaveBeenCalled())
    expect(container.querySelector('[data-testid="collision-prompts"]')).toBeNull()
  })
})
