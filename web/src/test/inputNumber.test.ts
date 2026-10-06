/**
 * The real number field against the `UInputNumber` stub in `mount.ts` (RD-1120-09).
 *
 * Every component test runs against the stub, so the stub sets the contract a cleared field is
 * tested with: what it hands its model when the field is emptied. This mounts Nuxt UI's own
 * `InputNumber.vue` over Reka's `NumberField` and records that value, so the stub cannot drift
 * from it unnoticed. The Nuxt build modules the component reads (`#imports`, `#build/…`) come from
 * `src/test/nuxtUi/` (`vitest.config.ts`); its plus and minus buttons are plain buttons here.
 */
import InputNumber from '@nuxt/ui/components/InputNumber.vue'
import { fireEvent, render } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import { uiStubs } from './mount'

vi.mock('@nuxt/ui/components/Button.vue', () => ({ default: { template: '<button type="button" />' } }))

/** Types `text` into the field and leaves it, the moment Reka commits the value. */
async function typeAndLeave(component: object, modelValue: number | null, text: string) {
  const view = render(component, { props: { modelValue } })
  const field = view.getByRole('spinbutton') as HTMLInputElement
  await fireEvent.update(field, text)
  await fireEvent.blur(field)
  return view.emitted<[unknown]>()['update:modelValue'] ?? []
}

describe('the number field', () => {
  it('hands its model undefined when it is emptied', async () => {
    expect(await typeAndLeave(InputNumber, 5, '')).toEqual([[undefined]])
  })

  it('hands its model the number typed', async () => {
    expect(await typeAndLeave(InputNumber, 5, '12')).toEqual([[12]])
  })

  // The stub commits on every keystroke, so a test needs no blur; the value is what counts.
  it('commits when it is left, not while it is typed in', async () => {
    const view = render(InputNumber, { props: { modelValue: 5 } })
    await fireEvent.update(view.getByRole('spinbutton'), '')
    expect(view.emitted()['update:modelValue']).toBeUndefined()
  })

  it('is stubbed with the same value for an emptied field', async () => {
    expect(await typeAndLeave(uiStubs.UInputNumber, 5, '')).toEqual([[undefined]])
  })
})
