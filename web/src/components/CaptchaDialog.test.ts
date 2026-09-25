import { fireEvent, render, screen } from '@testing-library/vue'
import axe from 'axe-core'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createI18n } from 'vue-i18n'

import type { PendingCaptcha } from '@/api/types'
import en from '@/locales/en/captcha.json'
import { useCaptchasStore } from '@/stores/captchas'

import CaptchaDialog from './CaptchaDialog.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn() },
  responseError: vi.fn(),
  resultMessage: vi.fn()
}))

const push = vi.fn()
vi.mock('vue-router', () => ({ useRouter: () => ({ push }) }))
const toast = { add: vi.fn() }
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => toast }))

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { captcha: en } } })

const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }
const components = {
  UModal: {
    props: ['open'],
    template: '<div v-if="open"><slot name="body" /><slot name="footer" /></div>'
  },
  UButton: {
    props: ['label', 'disabled', 'loading'],
    template: '<button v-bind="$attrs" :disabled="disabled">{{ label }}</button>'
  },
  UInput: {
    props: ['modelValue'],
    emits: ['update:modelValue'],
    template:
      '<input v-bind="$attrs" :value="modelValue" @input="$emit(\'update:modelValue\', $event.target.value)" />'
  },
  UAlert: {
    props: ['title', 'description'],
    template: '<div role="alert">{{ title }} {{ description }}</div>'
  },
  UFormField: passthrough,
  UBadge: passthrough
}

function imageCaptcha(): PendingCaptcha {
  return {
    id: 'captcha-1',
    kind: 'image',
    host: 'keep2share.cc',
    prompt: 'Type the code',
    image: 'data:image/png;base64,Qk0=',
    created_at: '2026-09-02T10:00:00Z',
    expires_at: '2026-09-02T10:03:00Z'
  } as PendingCaptcha
}

function widgetCaptcha(): PendingCaptcha {
  return {
    id: 'captcha-2',
    kind: 'turnstile',
    host: 'katfile.biz',
    site_key: '0x4AAA',
    page_url: 'https://katfile.biz/file',
    created_at: '2026-09-02T10:00:00Z',
    expires_at: '2026-09-02T10:03:00Z'
  } as PendingCaptcha
}

function clickCaptcha(): PendingCaptcha {
  return {
    id: 'captcha-3',
    kind: 'click_point',
    host: 'filecrypt.cc',
    prompt: 'Click the circle',
    image: 'data:image/png;base64,Qk0=',
    created_at: '2026-09-02T10:00:00Z',
    expires_at: '2026-09-02T10:03:00Z'
  } as PendingCaptcha
}

/** jsdom renders no pixels, so the image gets a size and a natural size by hand. */
function sizeImage(rendered: { width: number, height: number }, natural: { width: number, height: number }): void {
  vi.spyOn(HTMLImageElement.prototype, 'getBoundingClientRect').mockReturnValue({
    left: 10,
    top: 20,
    width: rendered.width,
    height: rendered.height,
    right: 10 + rendered.width,
    bottom: 20 + rendered.height,
    x: 10,
    y: 20,
    toJSON: () => ({})
  } as DOMRect)
  Object.defineProperty(HTMLImageElement.prototype, 'naturalWidth', { get: () => natural.width, configurable: true })
  Object.defineProperty(HTMLImageElement.prototype, 'naturalHeight', { get: () => natural.height, configurable: true })
}

function renderDialog(queue: PendingCaptcha[]) {
  const store = useCaptchasStore()
  store.pending = queue
  render(CaptchaDialog, { global: { plugins: [i18n], components } })
  return store
}

function button(label: string): HTMLButtonElement {
  return screen.getByRole('button', { name: label }) as HTMLButtonElement
}

describe('CaptchaDialog', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    vi.restoreAllMocks()
  })

  it('shows an image captcha with its picture and sends the typed answer', async () => {
    const store = renderDialog([imageCaptcha()])
    store.solve = vi.fn().mockResolvedValue({ ok: true, message: 'done' })

    expect(screen.getByAltText(en.image_alt).getAttribute('src')).toBe(
      'data:image/png;base64,Qk0='
    )
    expect(screen.getByText('Type the code')).toBeTruthy()

    await fireEvent.update(screen.getByPlaceholderText(en.answer_placeholder), '42')
    await fireEvent.click(button(en.submit))

    expect(store.solve).toHaveBeenCalledWith('captcha-1', '42')
  })

  /**
   * RD-110-15: a click-point captcha is answered by clicking the picture. The click is
   * translated into the image's own pixels — the dialog may have scaled the rendering — and
   * nothing is sent until the button is pressed, so a wrong spot can be moved first.
   */
  it('turns a click on the scaled picture into image pixels and sends it on request', async () => {
    sizeImage({ width: 200, height: 100 }, { width: 400, height: 200 })
    const store = renderDialog([clickCaptcha()])
    store.click = vi.fn().mockResolvedValue({ ok: true, message: 'done' })
    store.solve = vi.fn()

    expect(screen.getByText('Click the circle')).toBeTruthy()
    expect(screen.queryByPlaceholderText(en.answer_placeholder)).toBeNull()
    expect(screen.getByTestId('click-state').textContent).toContain(en.click.none)
    expect(button(en.submit).disabled).toBe(true)

    const image = screen.getByAltText(en.image_alt)
    await fireEvent.click(image, { clientX: 110, clientY: 70, detail: 1 })
    expect(screen.getByTestId('click-marker')).toBeTruthy()
    expect(screen.getByTestId('click-state').textContent).toContain('200, 100')
    expect(store.click).not.toHaveBeenCalled()

    // A second click moves the mark instead of sending the first one.
    await fireEvent.click(image, { clientX: 20, clientY: 30, detail: 1 })
    expect(screen.getByTestId('click-state').textContent).toContain('20, 20')

    expect(button(en.submit).disabled).toBe(false)
    await fireEvent.click(button(en.submit))
    expect(store.click).toHaveBeenCalledWith('captcha-3', 20, 20)
    expect(store.solve).not.toHaveBeenCalled()
  })

  /**
   * The keyboard route to the same answer (docs/accessibility.md, "Keyboard operation"): the
   * picture is a named button, the arrow keys move the mark from the centre of the image, Shift
   * makes the step ten pixels, the position is read out from a status line, and Enter sends it.
   */
  it('moves the mark with the arrow keys and sends it with Enter', async () => {
    sizeImage({ width: 200, height: 100 }, { width: 400, height: 200 })
    const store = renderDialog([clickCaptcha()])
    store.click = vi.fn().mockResolvedValue({ ok: true, message: 'done' })

    const surface = screen.getByRole('button', { name: en.click.surface })
    expect(surface.getAttribute('aria-describedby')).toBe('captcha-click-state')
    const state = screen.getByRole('status')
    expect(state.textContent).toContain(en.click.none)

    await fireEvent.keyDown(surface, { key: 'ArrowRight' })
    expect(state.textContent).toContain('201, 100')
    await fireEvent.keyDown(surface, { key: 'ArrowRight' })
    await fireEvent.keyDown(surface, { key: 'ArrowDown', shiftKey: true })
    await fireEvent.keyDown(surface, { key: 'ArrowLeft', shiftKey: true })
    expect(state.textContent).toContain('192, 110')
    expect(store.click).not.toHaveBeenCalled()

    await fireEvent.keyDown(surface, { key: 'Enter' })
    expect(store.click).toHaveBeenCalledWith('captcha-3', 192, 110)
  })

  it('never sends anything from the keyboard while no spot is marked', async () => {
    sizeImage({ width: 200, height: 100 }, { width: 400, height: 200 })
    const store = renderDialog([clickCaptcha()])
    store.click = vi.fn()

    await fireEvent.keyDown(screen.getByRole('button', { name: en.click.surface }), { key: ' ' })

    expect(store.click).not.toHaveBeenCalled()
  })

  it('renders a click-point captcha without an axe violation', async () => {
    const store = useCaptchasStore()
    store.pending = [clickCaptcha()]
    const { container } = render(CaptchaDialog, { global: { plugins: [i18n], components } })

    // `region` is a property of the page shell, not of a component; see accessibility.test.ts.
    const results = await axe.run(container, { rules: { region: { enabled: false } } })
    expect(results.violations.map(v => `${v.id}: ${v.help}`).join('\n')).toBe('')
  })

  it('keeps the answer button out of reach until something is typed', () => {
    renderDialog([imageCaptcha()])

    expect(button(en.submit).disabled).toBe(true)
  })

  /**
   * The rule the feature rests on: a widget belongs to the hoster's domain, so the dialog
   * explains the stall and points at the settings instead of trying to render it.
   */
  it('never renders a widget challenge, only the hint and a way to configure a solver', async () => {
    renderDialog([widgetCaptcha()])

    expect(screen.queryByAltText(en.image_alt)).toBeNull()
    expect(screen.queryByRole('button', { name: en.submit })).toBeNull()
    // Two alerts now: the widget explanation and the extension state (RD-108-02).
    expect(screen.getAllByRole('alert')[0]?.textContent).toContain('katfile.biz')

    await fireEvent.click(button(en.widget.configure))
    expect(push).toHaveBeenCalledWith('/settings/captcha')
  })

  /**
   * RD-108-02: the interface cannot know whether an extension is installed, so it asks the
   * server as soon as a widget shows, and says which of the two truths applies.
   */
  it('asks who can answer a widget and offers the extension setup while none is connected', async () => {
    const store = useCaptchasStore()
    store.refreshAnswerers = vi.fn().mockResolvedValue(undefined)
    store.answerers = { browser_extension_connected: false }
    renderDialog([widgetCaptcha()])

    expect(store.refreshAnswerers).toHaveBeenCalledTimes(1)
    expect(screen.getByTestId('extension-missing').textContent).toContain(en.widget.extension_missing)
    expect(screen.queryByTestId('extension-connected')).toBeNull()

    await fireEvent.click(button(en.widget.extension_setup))
    expect(push).toHaveBeenCalledWith('/settings/desktop')
  })

  it('says the extension will open the hoster page once the server has seen one', () => {
    const store = useCaptchasStore()
    store.refreshAnswerers = vi.fn().mockResolvedValue(undefined)
    store.answerers = {
      browser_extension_connected: true,
      browser_extension_seen_at: '2026-09-16T12:00:00Z'
    }
    renderDialog([widgetCaptcha()])

    expect(screen.getByTestId('extension-connected').textContent).toContain('katfile.biz')
    expect(screen.queryByTestId('extension-missing')).toBeNull()
    expect(screen.queryByRole('button', { name: en.widget.extension_setup })).toBeNull()
  })

  it('does not ask about the extension for an image captcha', () => {
    const store = useCaptchasStore()
    store.refreshAnswerers = vi.fn().mockResolvedValue(undefined)
    renderDialog([imageCaptcha()])

    expect(store.refreshAnswerers).not.toHaveBeenCalled()
    expect(screen.queryByTestId('extension-missing')).toBeNull()
  })

  it('lets any challenge be declined', async () => {
    const store = renderDialog([widgetCaptcha()])
    store.skip = vi.fn().mockResolvedValue({ ok: true, message: 'declined' })

    await fireEvent.click(button(en.skip))

    expect(store.skip).toHaveBeenCalledWith('captcha-2')
  })

  it('counts down towards the deadline the server set', () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-09-02T10:01:00Z'))
    try {
      renderDialog([imageCaptcha()])
      expect(screen.getByText('Expires in 2:00')).toBeTruthy()
    } finally {
      vi.useRealTimers()
    }
  })

  it('says so when the queue could not be loaded instead of failing silently', () => {
    const store = useCaptchasStore()
    store.pending = [imageCaptcha()]
    store.error = 'The captcha queue is unreachable'
    render(CaptchaDialog, { global: { plugins: [i18n], components } })

    expect(screen.getByRole('alert').textContent).toContain('unreachable')
  })

  it('shows nothing at all while no captcha is waiting', () => {
    renderDialog([])

    expect(screen.queryByRole('button', { name: en.skip })).toBeNull()
  })
})
