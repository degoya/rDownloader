/**
 * The date field with its calendar (RD-1140-09): a day typed into the field and a day picked in
 * the calendar reach the model alike, as the `YYYY-MM-DD` the settings keep; the bounds and the
 * disabled state reach both, and both speak the interface's language.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import common from '@/locales/en/common.json'
import { axeViolations } from '@/test/axe'
import { mountComponent, openablePopover } from '@/test/mount'

import DateField from './DateField.vue'

const CALENDAR = common.date_field.open_calendar

function renderField(props: Record<string, unknown> = {}, locale = 'en') {
  return mountComponent(DateField, {
    props: { modelValue: '2026-03-05', 'aria-label': 'Reset day', ...props },
    stubs: { UPopover: openablePopover },
    locale
  })
}

const field = () => screen.getByLabelText('Reset day') as HTMLInputElement

describe('DateField', () => {
  it('shows the day it holds and hands a typed one to its model', async () => {
    const { emitted } = renderField()
    expect(field().value).toBe('2026-03-05')
    await fireEvent.update(field(), '2026-04-01')
    await fireEvent.update(field(), '')
    expect(emitted()['update:modelValue']).toEqual([['2026-04-01'], ['']])
  })

  it('opens a calendar at its button and sets the day picked there, then closes it', async () => {
    const { emitted } = renderField()
    expect(screen.queryByRole('group')).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: CALENDAR }))
    await fireEvent.click(screen.getByRole('button', { name: '2026-03-12' }))
    expect(emitted()['update:modelValue']).toEqual([['2026-03-12']])
    expect(screen.queryByRole('group')).toBeNull()
  })

  it('opens the calendar on an empty field too', async () => {
    const { emitted } = renderField({ modelValue: '', min: '2027-01-10' })
    expect(field().value).toBe('')
    await fireEvent.click(screen.getByRole('button', { name: CALENDAR }))
    await fireEvent.click(screen.getByRole('button', { name: '2027-01-20' }))
    expect(emitted()['update:modelValue']).toEqual([['2027-01-20']])
  })

  it('hands its bounds to the field and to the calendar', async () => {
    renderField({ min: '2026-03-03', max: '2026-03-20' })
    expect(field().getAttribute('min')).toBe('2026-03-03')
    expect(field().getAttribute('max')).toBe('2026-03-20')
    await fireEvent.click(screen.getByRole('button', { name: CALENDAR }))
    const day = (name: string) => screen.getByRole('button', { name }) as HTMLButtonElement
    expect(day('2026-03-02').disabled).toBe(true)
    expect(day('2026-03-03').disabled).toBe(false)
    expect(day('2026-03-20').disabled).toBe(false)
    expect(day('2026-03-21').disabled).toBe(true)
  })

  it('takes no day while disabled, typed or picked', () => {
    renderField({ disabled: true })
    expect(field().disabled).toBe(true)
    expect((screen.getByRole('button', { name: CALENDAR }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('speaks the interface language, for the order of the segments, the week start and the month names', async () => {
    renderField({}, 'de')
    expect(field().lang).toBe('de')
    await fireEvent.click(screen.getByRole('button', { name: CALENDAR }))
    expect(screen.getByRole('group').lang).toBe('de')
  })

  it('has no accessibility violations, closed and open', async () => {
    const { container } = renderField()
    expect(await axeViolations(container)).toBe('')
    await fireEvent.click(screen.getByRole('button', { name: CALENDAR }))
    expect(await axeViolations(container)).toBe('')
  })
})
