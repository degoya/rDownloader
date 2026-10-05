import { fireEvent, render, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createI18n } from 'vue-i18n'

import routing from '@/locales/en/routing.json'
import { fileUpload } from '@/test/mount'

import RoutingBackupButtons from './RoutingBackupButtons.vue'

const get = vi.fn()
const post = vi.fn()
const confirm = vi.fn()
vi.mock('@/api/client', () => ({
  api: { GET: (...args: unknown[]) => get(...args), POST: (...args: unknown[]) => post(...args) },
  responseError: vi.fn()
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirm }))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { routing } } })

/** Renders the menu's items as plain buttons, so each export is one click away in the test. */
const UDropdownMenu = {
  props: ['items'],
  template: '<div><slot /><button v-for="item in items" :key="item.label" @click="item.onSelect()">{{ item.label }}</button></div>'
}

function mount() {
  return render(RoutingBackupButtons, {
    global: {
      plugins: [i18n],
      stubs: { UDropdownMenu, UFileUpload: fileUpload, UButton: { props: ['label'], template: '<button>{{ label }}</button>' } }
    }
  })
}

describe('RoutingBackupButtons', () => {
  beforeEach(() => {
    get.mockReset()
    get.mockResolvedValue({ data: { format: 'rdownloader-routing-bundle', version: 1, categories: [], rules: [] } })
    URL.createObjectURL = vi.fn(() => 'blob:x')
    URL.revokeObjectURL = vi.fn()
  })

  it.each([
    [routing.backup.export_all, 'all'],
    [routing.backup.export_categories, 'categories'],
    [routing.backup.export_rules, 'rules']
  ])('"%s" exports the part "%s"', async (label, part) => {
    mount()
    await fireEvent.click(screen.getByText(label))
    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/routing/export', { params: { query: { part } } }))
  })

  it('imports a chosen routing bundle once the confirmation is given (RD-1110-12)', async () => {
    const bundle = { format: 'rdownloader-routing-bundle', version: 1, categories: [], rules: [] }
    confirm.mockResolvedValue(true)
    post.mockResolvedValue({ data: { categories_created: 0, rules_created: 0, categories_skipped: 0, rules_skipped: 0 } })
    const { container } = mount()

    const input = container.querySelector('input[type="file"]') as HTMLInputElement
    expect(input.accept).toBe('.json')
    Object.defineProperty(input, 'files', { value: [new File([JSON.stringify(bundle)], 'routing.json')], configurable: true })
    await fireEvent.change(input)

    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/routing/import', { body: bundle }))
  })
})
