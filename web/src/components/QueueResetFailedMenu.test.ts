/**
 * "Reset failed" in the Downloads header (RD-1190-15): the three kinds with their numbers, the
 * dead entries and button where there is nothing to reset, and what a pick reports.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import downloads from '@/locales/en/downloads.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import QueueResetFailedMenu from './QueueResetFailedMenu.vue'

vi.mock('@nuxt/ui/components/Icon.vue', () => ({ default: { template: '<span />' } }))

function mount(counts = { failed: 2, blocked: 1, both: 3 }) {
  const onReset = vi.fn()
  const view = mountComponent(QueueResetFailedMenu, { props: { counts, onReset }, messages: { downloads } })
  return { view, onReset }
}

describe('QueueResetFailedMenu', () => {
  it('offers failed, blocked and both, each with its number', async () => {
    const { onReset } = mount()
    await fireEvent.click(screen.getByRole('button', { name: 'Blocked (1)' }))
    await fireEvent.click(screen.getByRole('button', { name: 'Failed and blocked (3)' }))
    expect(onReset.mock.calls).toEqual([['blocked'], ['both']])
    expect(screen.getByRole('button', { name: 'Failed (2)' })).toBeTruthy()
  })

  it('is dead where there is nothing to reset', () => {
    mount({ failed: 0, blocked: 2, both: 2 })
    expect((screen.getByRole('button', { name: 'Failed (0)' }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole('button', { name: 'Reset failed' }) as HTMLButtonElement).disabled).toBe(false)
  })

  it('disables the button while the list holds neither', () => {
    mount({ failed: 0, blocked: 0, both: 0 })
    expect((screen.getByRole('button', { name: 'Reset failed' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('has no accessibility violations', async () => {
    const { view } = mount()
    expect(await axeViolations(view.container)).toBe('')
  })
})
