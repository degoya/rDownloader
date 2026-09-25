import { fireEvent, render, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, ref } from 'vue'
import { createI18n } from 'vue-i18n'

import { api } from '@/api/client'
import en from '@/locales/en/captcha.json'

import SettingsCaptchaCard from './SettingsCaptchaCard.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), PUT: vi.fn(), POST: vi.fn() },
  responseError: vi.fn(() => 'The solver rejected the key')
}))

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { captcha: en } } })

const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }
const model = {
  props: ['modelValue'],
  emits: ['update:modelValue'],
  template:
    '<input v-bind="$attrs" :value="modelValue" @input="$emit(\'update:modelValue\', $event.target.value)" />'
}
const components = {
  UButton: {
    props: ['label', 'disabled', 'loading'],
    template: '<button v-bind="$attrs" :disabled="disabled">{{ label }}</button>'
  },
  UInput: {
    ...model,
    template:
      '<input v-bind="$attrs" :value="modelValue" @input="$emit(\'update:modelValue\', $event.target.value)" /><slot name="trailing" />'
  },
  USelect: model,
  USwitch: {
    props: ['modelValue'],
    emits: ['update:modelValue'],
    template:
      '<input type="checkbox" v-bind="$attrs" :checked="modelValue" @change="$emit(\'update:modelValue\', $event.target.checked)" />'
  },
  UAlert: { props: ['title'], template: '<div role="alert">{{ title }}</div>' },
  UFormField: passthrough,
  UBadge: passthrough
}

const storedConfig = {
  solver: 'two_captcha_compatible',
  endpoint: 'https://api.2captcha.com',
  has_api_key: true,
  manual_enabled: true,
  manual_timeout_seconds: 180
}

const Harness = defineComponent({
  components: { SettingsCaptchaCard },
  setup() {
    const card = ref<{ save: () => Promise<boolean> } | null>(null)
    const save = (): Promise<boolean> | undefined => card.value?.save()
    return { card, save }
  },
  template: '<SettingsCaptchaCard ref="card" /><button @click="save">Save configuration</button>'
})

function renderCard() {
  return render(Harness, { global: { plugins: [i18n], components } })
}

/** The timeout is the only number field on the card. */
function timeoutField(): HTMLInputElement {
  return screen.getByRole('spinbutton') as HTMLInputElement
}

async function loaded() {
  renderCard()
  await waitFor(() => expect(api.GET).toHaveBeenCalledWith('/api/v1/captcha-config'))
  await waitFor(() => expect(timeoutField().value).toBe('180'))
}

describe('SettingsCaptchaCard', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(api.GET).mockResolvedValue({ data: { ...storedConfig } } as never)
    vi.mocked(api.PUT).mockResolvedValue({ data: { ...storedConfig } } as never)
  })

  /** The API clamps the timeout to this range, so the field must not invite other values. */
  it('offers only the answer timeouts the API actually accepts', async () => {
    await loaded()

    expect(timeoutField().getAttribute('min')).toBe('15')
    expect(timeoutField().getAttribute('max')).toBe('600')
  })

  it('sends only the fields the user actually filled in', async () => {
    await loaded()

    await fireEvent.click(screen.getByRole('button', { name: 'Save configuration' }))

    await waitFor(() => expect(api.PUT).toHaveBeenCalled())
    const [, options] = vi.mocked(api.PUT).mock.calls[0] as unknown as [
      string,
      { body: Record<string, unknown> }
    ]
    expect(options.body).not.toHaveProperty('api_key')
    expect(options.body).not.toHaveProperty('clear_api_key')
    expect(options.body.manual_timeout_seconds).toBe(180)
  })

  it('never keeps the typed key in the form after saving it', async () => {
    await loaded()
    const key = screen.getByPlaceholderText('••••••••') as HTMLInputElement
    await fireEvent.update(key, 'brand-new-key')

    await fireEvent.click(screen.getByRole('button', { name: 'Save configuration' }))

    await waitFor(() => expect(api.PUT).toHaveBeenCalled())
    const [, options] = vi.mocked(api.PUT).mock.calls[0] as unknown as [
      string,
      { body: Record<string, unknown> }
    ]
    expect(options.body.api_key).toBe('brand-new-key')
    await waitFor(() => expect(key.value).toBe(''))
  })

  it('tests the key that is on screen, so a wrong one need never be saved', async () => {
    await loaded()
    vi.mocked(api.POST).mockResolvedValue({ data: { balance: 12.5 } } as never)
    await fireEvent.update(screen.getByPlaceholderText('••••••••'), 'unsaved-key')

    await fireEvent.click(screen.getByRole('button', { name: en.settings.test.button }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/captcha-config/test', {
        body: { endpoint: 'https://api.2captcha.com', api_key: 'unsaved-key' }
      })
    )
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('12.5'))
  })

  it('explains a refused key instead of leaving the user guessing', async () => {
    await loaded()
    vi.mocked(api.POST).mockResolvedValue({ error: { code: 'captcha.solver_failed' } } as never)

    await fireEvent.click(screen.getByRole('button', { name: en.settings.test.button }))

    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toContain('The solver rejected the key')
    )
  })

  it('cannot test a solver that has no key at all', async () => {
    vi.mocked(api.GET).mockResolvedValue({
      data: { ...storedConfig, solver: 'none', has_api_key: false }
    } as never)
    await renderCard()
    await waitFor(() => expect(api.GET).toHaveBeenCalled())

    await waitFor(() =>
      expect(
        (screen.getByRole('button', { name: en.settings.test.button }) as HTMLButtonElement).disabled
      ).toBe(true)
    )
  })
})
