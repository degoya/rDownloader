import { fireEvent, render, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createI18n } from 'vue-i18n'

import routing from '@/locales/en/routing.json'

import RoutingBackupButtons from './RoutingBackupButtons.vue'

const get = vi.fn()
vi.mock('@/api/client', () => ({
  api: { GET: (...args: unknown[]) => get(...args), POST: vi.fn() },
  responseError: vi.fn()
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn() }))
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
      stubs: { UDropdownMenu, UButton: { props: ['label'], template: '<button>{{ label }}</button>' } }
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
})
