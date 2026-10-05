/**
 * The copy row (RD-1110-13): the value in a read-only field, a button that copies exactly that
 * value and says so for two seconds, and nothing announced when the clipboard refused.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import common from '@/locales/en/common.json'
import { mountComponent } from '@/test/mount'

const copyText = vi.fn<(text: string) => Promise<boolean>>()
vi.mock('@/composables/useCopy', () => ({ useCopy: () => copyText }))

const { default: CopyField } = await import('./CopyField.vue')

describe('CopyField', () => {
  beforeEach(() => {
    copyText.mockReset()
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('shows the value in a read-only field named by the label', () => {
    mountComponent(CopyField, { props: { value: 'Bearer rdp_secret', label: 'Copy header' } })

    const field = screen.getByLabelText('Copy header') as HTMLInputElement
    expect(field.value).toBe('Bearer rdp_secret')
    expect(field.hasAttribute('readonly')).toBe(true)
  })

  it('copies the value, says so on the button for two seconds and tells the caller', async () => {
    copyText.mockResolvedValue(true)
    const { emitted } = mountComponent(CopyField, { props: { value: 'rdp_secret', label: 'Copy token' } })

    await fireEvent.click(screen.getByRole('button', { name: 'Copy token' }))
    await vi.waitFor(() => screen.getByRole('button', { name: common.copy.copied }))

    expect(copyText).toHaveBeenCalledWith('rdp_secret')
    expect(emitted().copied).toHaveLength(1)
    await vi.advanceTimersByTimeAsync(2000)
    expect(screen.getByRole('button', { name: 'Copy token' })).toBeTruthy()
  })

  it('claims nothing when the clipboard refused', async () => {
    copyText.mockResolvedValue(false)
    const { emitted } = mountComponent(CopyField, { props: { value: 'rdp_secret', label: 'Copy token' } })

    await fireEvent.click(screen.getByRole('button', { name: 'Copy token' }))
    await vi.advanceTimersByTimeAsync(0)

    expect(copyText).toHaveBeenCalledWith('rdp_secret')
    expect(screen.queryByRole('button', { name: common.copy.copied })).toBeNull()
    expect(emitted().copied).toBeUndefined()
  })

  it('names an icon-only button by its label and tooltip', async () => {
    copyText.mockResolvedValue(true)
    mountComponent(CopyField, { props: { value: 'https://dl.example.com/cb', label: 'Copy', iconOnly: true } })

    const button = screen.getByRole('button', { name: 'Copy' })
    expect(button.textContent).toBe('')
    expect(button.getAttribute('title')).toBe('Copy')
    await fireEvent.click(button)
    await vi.waitFor(() => expect(button.getAttribute('aria-label')).toBe(common.copy.copied))
  })
})
