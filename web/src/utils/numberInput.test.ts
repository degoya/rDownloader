/**
 * The number fields as Reka parses them under our format options (RD-1110-10).
 *
 * `UInputNumber` itself resolves through `#imports` and cannot be mounted here, so this mounts the
 * Reka number field it wraps, under the locale `UApp` hands down, with the options and the
 * `orNull` the interface uses: an emptied field, a value above `max` and a German decimal comma.
 */
import { fireEvent, render, screen } from '@testing-library/vue'
import { ConfigProvider, NumberFieldInput, NumberFieldRoot } from 'reka-ui'
import { describe, expect, it } from 'vitest'
import { defineComponent, h, ref } from 'vue'

import { DECIMAL, PLAIN, WHOLE, isNumber, orNull } from './numberInput'

function field(locale: string, options: Intl.NumberFormatOptions, initial: number | null, bounds: { min?: number, max?: number } = {}) {
  const value = ref<number | null>(initial)
  const Harness = defineComponent(() => () => h(ConfigProvider, { locale }, () => h(NumberFieldRoot, {
    modelValue: value.value,
    formatOptions: options,
    stepSnapping: false,
    ...bounds,
    'onUpdate:modelValue': (next: number | null | undefined) => { value.value = orNull(next) }
  }, () => h(NumberFieldInput))))
  render(Harness)
  return { value, input: screen.getByRole('spinbutton') as HTMLInputElement }
}

async function type(input: HTMLInputElement, text: string): Promise<void> {
  await fireEvent.update(input, text)
  await fireEvent.blur(input)
}

describe('the number field', () => {
  it('turns an emptied field into null', async () => {
    const { value, input } = field('en', WHOLE, 8)
    await type(input, '')
    expect(value.value).toBeNull()
  })

  it('holds a value above max at max', async () => {
    const { value, input } = field('en', WHOLE, 8, { min: 1, max: 32 })
    await type(input, '500')
    expect(value.value).toBe(32)
  })

  it('reads a German decimal comma and shows it back', async () => {
    const { value, input } = field('de', DECIMAL, null)
    await type(input, '1,5')
    expect(value.value).toBe(1.5)
    expect(input.value).toBe('1,5')
  })

  it('rounds a typed fraction in a whole-number field', async () => {
    const { value, input } = field('de', WHOLE, null)
    await type(input, '2,6')
    expect(value.value).toBe(3)
  })

  it('writes a port without a thousands separator', () => {
    const { input } = field('de', PLAIN, 65535)
    expect(input.value).toBe('65535')
  })
})

describe('orNull and isNumber', () => {
  it('say "not set" with null and accept only finite numbers', () => {
    expect(orNull(undefined)).toBeNull()
    expect(orNull(null)).toBeNull()
    expect(orNull(Number.NaN)).toBeNull()
    expect(orNull(0)).toBe(0)
    expect(isNumber(undefined)).toBe(false)
    expect(isNumber(3)).toBe(true)
  })
})
