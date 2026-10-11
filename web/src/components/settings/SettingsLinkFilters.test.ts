/**
 * The LinkFilter rules under Settings › LinkGrabber (RD-1240-09): written in the form beside the
 * list, ordered with the arrows, switched in the row, and applied to the LinkGrabber on request.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import settings from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
const post = vi.fn()
const put = vi.fn()
const apply = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
    PUT: (...args: unknown[]) => put(...args),
    PATCH: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: vi.fn(() => 'The service did not answer'),
  resultMessage: vi.fn(() => '')
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => async () => true }))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))
vi.mock('@/stores/categories', () => ({
  useCategories: () => ({ categories: ref([{ id: 'cat-1', name: 'Extras' }]), fetchCategories: vi.fn() })
}))
vi.mock('@/stores/collector', () => ({
  useCollectorStore: () => ({ applyLinkFilters: (...args: unknown[]) => apply(...args), error: null })
}))

const { default: SettingsLinkFilters } = await import('./SettingsLinkFilters.vue')

function stored(id: string, position: number, overrides: Record<string, unknown> = {}) {
  return {
    id,
    name: `Rule ${position}`,
    position,
    enabled: true,
    action: 'hide',
    name_pattern: '*.nfo',
    name_syntax: 'glob',
    extensions: [],
    ...overrides
  }
}

function mount() {
  return mountComponent(SettingsLinkFilters, { messages: { settings } })
}

describe('SettingsLinkFilters', () => {
  beforeEach(() => {
    get.mockReset()
    post.mockReset()
    put.mockReset()
    apply.mockReset()
  })

  it('creates a hiding rule from the form and lists it with what it checks', async () => {
    get.mockResolvedValue({ data: [] })
    post.mockResolvedValue({ data: stored('r1', 1, { name: 'Info files' }) })
    const { container } = mount()
    await screen.findByText(settings.link_filters.empty)

    await fireEvent.update(screen.getByTestId('link-filter-name'), ' Info files ')
    await fireEvent.update(screen.getByTestId('link-filter-pattern'), '*.nfo')
    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)

    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/link-filters', {
      body: {
        name: 'Info files',
        enabled: true,
        action: 'hide',
        name_pattern: '*.nfo',
        name_syntax: 'glob',
        size_min: null,
        size_max: null,
        extensions: [],
        hoster: null,
        source: null,
        package_name: null,
        category_id: null
      }
    }))
    const rows = await screen.findAllByTestId('link-filter-row')
    expect(rows).toHaveLength(1)
    expect(rows[0]?.textContent).toContain('*.nfo')
  })

  it('moves a rule one step and takes the order the server answers', async () => {
    get.mockResolvedValue({ data: [stored('r1', 1, { name: 'First' }), stored('r2', 2, { name: 'Second' })] })
    post.mockResolvedValue({ data: [stored('r2', 1, { name: 'Second' }), stored('r1', 2, { name: 'First' })] })
    mount()
    await screen.findAllByTestId('link-filter-row')

    const [down] = screen.getAllByRole('button', { name: settings.link_filters.move_down })
    await fireEvent.click(down as HTMLElement)

    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/link-filters/reorder', { body: { ids: ['r2', 'r1'] } }))
    await waitFor(() => expect(screen.getAllByTestId('link-filter-row')[0]?.textContent).toContain('Second'))
  })

  it('names a route rule\'s package and category in its row', async () => {
    get.mockResolvedValue({ data: [stored('r1', 1, { action: 'route', package_name: 'Info', category_id: 'cat-1' })] })
    mount()

    const [row] = await screen.findAllByTestId('link-filter-row')
    expect(row?.textContent).toContain('→ Info · Extras')
  })

  it('applies the rules to the LinkGrabber on request', async () => {
    get.mockResolvedValue({ data: [stored('r1', 1)] })
    apply.mockResolvedValue({ hidden: 1, shown: 0, routed: 0 })
    mount()
    await screen.findAllByTestId('link-filter-row')

    await fireEvent.click(screen.getByTestId('link-filters-apply'))
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1))
  })
})
