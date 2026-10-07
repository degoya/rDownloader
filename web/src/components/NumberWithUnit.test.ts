/**
 * The number field with its unit attached (RD-1140-08): the unit is an outline badge in the
 * field's own group, every attribute but `class` reaches the number field, and `class` sizes the
 * group the two share.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import { mountComponent, unitOf } from '@/test/mount'

import NumberWithUnit from './NumberWithUnit.vue'

describe('NumberWithUnit', () => {
  it('sets the unit beside the field, in one group', () => {
    mountComponent(NumberWithUnit, { props: { modelValue: 5, unit: 'MiB/s' } })

    const field = screen.getByRole('spinbutton') as HTMLInputElement
    expect(field.value).toBe('5')
    expect(unitOf(field)).toBe('MiB/s')
  })

  it('hands the bounds and the steppers to the field and sizes the group', () => {
    mountComponent(NumberWithUnit, {
      props: { modelValue: 3, unit: 'h', min: 1, max: 24, required: true, increment: '', decrement: '', class: 'mt-2 w-full', 'data-testid': 'hours' }
    })

    const field = screen.getByTestId('hours') as HTMLInputElement
    expect(field.min).toBe('1')
    expect(field.max).toBe('24')
    expect(field.required).toBe(true)
    expect(field.hasAttribute('data-steppers')).toBe(true)
    expect(field.classList.contains('w-full')).toBe(false)
    const group = field.closest('[data-number-unit]') as HTMLElement
    expect(group.classList.contains('w-full')).toBe(true)
    expect(group.classList.contains('mt-2')).toBe(true)
  })

  it('reports a typed number and an emptied field to its model', async () => {
    const update = vi.fn()
    mountComponent(NumberWithUnit, { props: { modelValue: 5, unit: 's', 'onUpdate:modelValue': update } })
    const field = screen.getByRole('spinbutton')

    await fireEvent.update(field, '42')
    await fireEvent.update(field, '')

    expect(update.mock.calls).toEqual([[42], [undefined]])
  })
})
