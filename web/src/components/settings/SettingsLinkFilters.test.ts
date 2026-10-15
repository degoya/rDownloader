/**
 * The LinkFilter rules under Settings › LinkGrabber (RD-1240-09): written in the form beside the
 * list, ordered with the arrows, switched in the row, and applied to the LinkGrabber on request.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock, confirmMock, toastMock } from '@/test/mocks'
import { ref } from 'vue'

import linkgrabberDe from '@/locales/de/linkgrabber.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import linkgrabberEs from '@/locales/es/linkgrabber.json'
import linkgrabberFr from '@/locales/fr/linkgrabber.json'
import settings from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
const post = vi.fn()
const put = vi.fn()
const apply = vi.fn()
const confirm = vi.fn()
vi.mock('@/api/client', () => apiClientMock({
  GET: (...args: unknown[]) => get(...args),
  POST: (...args: unknown[]) => post(...args),
  PUT: (...args: unknown[]) => put(...args)
}))
vi.mock('@/composables/useConfirm', () => confirmMock((...args: unknown[]) => confirm(...args)))
vi.mock('@nuxt/ui/composables', () => toastMock())
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
  return mountComponent(SettingsLinkFilters, { messages: { settings, linkgrabber } })
}

describe('SettingsLinkFilters', () => {
  beforeEach(() => {
    get.mockReset()
    post.mockReset()
    put.mockReset()
    apply.mockReset()
    confirm.mockReset()
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
        without_resolver: false,
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

  it('applies the rules to the LinkGrabber on request, without asking when nothing is deleted', async () => {
    get.mockResolvedValue({ data: [stored('r1', 1)] })
    apply.mockResolvedValue({ hidden: 1, shown: 0, routed: 0, deleted: 0 })
    mount()
    await screen.findAllByTestId('link-filter-row')

    await fireEvent.click(screen.getByTestId('link-filters-apply'))
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(2))
    expect(apply).toHaveBeenNthCalledWith(1, true)
    expect(apply).toHaveBeenNthCalledWith(2)
    expect(confirm).not.toHaveBeenCalled()
  })

  it('asks with the count before a delete rule deletes, and applies nothing when declined (RD-1310-01)', async () => {
    get.mockResolvedValue({ data: [stored('r1', 1, { action: 'delete', name_pattern: null, without_resolver: true, source: 'clipboard' })] })
    apply.mockResolvedValue({ hidden: 0, shown: 0, routed: 0, deleted: 3 })
    confirm.mockResolvedValue(false)
    mount()
    const [row] = await screen.findAllByTestId('link-filter-row')
    expect(row?.textContent).toContain(settings.link_filters.actions.delete)
    expect(row?.textContent).toContain(settings.link_filters.without_resolver_token)

    await fireEvent.click(screen.getByTestId('link-filters-apply'))
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    expect(confirm.mock.calls[0]?.[0]).toMatchObject({
      description: '3 links will be deleted from the LinkGrabber by a delete rule.',
      destructive: true
    })
    expect(apply).toHaveBeenCalledTimes(1)
    expect(apply).toHaveBeenCalledWith(true)

    confirm.mockResolvedValue(true)
    await fireEvent.click(screen.getByTestId('link-filters-apply'))
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(3))
    expect(apply).toHaveBeenLastCalledWith()
  })

  it('sends the without-resolver switch with the rule', async () => {
    get.mockResolvedValue({ data: [] })
    post.mockResolvedValue({ data: stored('r1', 1, { name: 'Pages', without_resolver: true }) })
    const { container } = mount()
    await screen.findByText(settings.link_filters.empty)

    await fireEvent.update(screen.getByTestId('link-filter-name'), 'Pages')
    await fireEvent.click(screen.getByTestId('link-filter-without-resolver'))
    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)

    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/link-filters', {
      body: expect.objectContaining({ name: 'Pages', without_resolver: true })
    }))
  })

  it('no longer promises in any language that applying deletes nothing (RD-1310-01)', () => {
    for (const catalogue of [linkgrabberDe, linkgrabber, linkgrabberEs, linkgrabberFr]) {
      expect(catalogue.link_filter.apply_hint).not.toMatch(/Nichts wird gelöscht|Nothing is deleted|No se elimina nada|Rien n'est supprimé/)
      expect(catalogue.link_filter.applied).toContain('{deleted}')
    }
  })
})
