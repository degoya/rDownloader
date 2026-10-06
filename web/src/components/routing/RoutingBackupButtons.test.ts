import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import routing from '@/locales/en/routing.json'
import { mountComponent } from '@/test/mount'

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

function mount() {
  return mountComponent(RoutingBackupButtons, { messages: { routing } })
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
