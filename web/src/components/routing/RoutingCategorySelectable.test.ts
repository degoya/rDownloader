/**
 * The category editor's "Show in the selection" switch (RD-1260-03): off is sent as
 * `selectable: false`, the default category cannot be switched off, and a hidden category says
 * so in the list.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock, confirmMock, eventStreamMock } from '@/test/mocks'

import type { Category, StorageRoot } from '@/api/types'
import common from '@/locales/en/common.json'
import routing from '@/locales/en/routing.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
const post = vi.fn()
const put = vi.fn()
vi.mock('@/api/client', () => apiClientMock({
  GET: (...args: unknown[]) => get(...args),
  POST: (...args: unknown[]) => post(...args),
  PUT: (...args: unknown[]) => put(...args),
  responseError: vi.fn(() => 'rejected')
}))
vi.mock('@/composables/useConfirm', () => confirmMock())
vi.mock('@/composables/useEventStream', () => eventStreamMock())

const { default: RoutingCategories } = await import('./RoutingCategories.vue')

const ROOT: StorageRoot = {
  id: 'root-1',
  name: 'Downloads',
  path: '/downloads',
  is_default: true,
  minimum_free_bytes: null,
  persistence: 'persistent'
}

function category(id: string, name: string, extra: Partial<Category> = {}): Category {
  return {
    id,
    name,
    color: '#38BDF8',
    storage_root_id: ROOT.id,
    relative_path: name.toLowerCase(),
    is_default: false,
    selectable: true,
    ...extra
  } as Category
}

function mount(categories: Category[]) {
  return mountComponent(RoutingCategories, {
    messages: { routing },
    props: { modelValue: categories, roots: [ROOT], loading: false, loadError: null },
    stubs: { UInputTags: true }
  })
}

function selectableSwitch(): HTMLElement {
  return screen.getByRole('switch', { name: routing.category.selectable_label })
}

function rowOf(name: string): HTMLElement {
  return screen.getByText(name).closest('div.border') as HTMLElement
}

describe('RoutingCategories selectable switch', () => {
  beforeEach(() => {
    get.mockReset()
    get.mockImplementation(async (path: string) =>
      path === '/api/v1/postprocess/scripts' ? { data: { scripts: [], directory: '/scripts' } } : { data: [] }
    )
    post.mockReset()
    put.mockReset()
  })

  it('is on for a new category and sends it switched off', async () => {
    mount([category('cat-1', 'Films', { is_default: true })])
    expect(selectableSwitch().getAttribute('aria-checked')).toBe('true')
    post.mockResolvedValue({ data: category('cat-9', 'Automation', { selectable: false }) })

    await fireEvent.update(screen.getByPlaceholderText(routing.category.name_placeholder), 'Automation')
    await fireEvent.click(selectableSwitch())
    await fireEvent.click(screen.getByRole('button', { name: routing.category.create }))

    await waitFor(() => expect(post).toHaveBeenCalled())
    expect(post.mock.calls[0]?.[1]?.body).toMatchObject({ name: 'Automation', is_default: false, selectable: false })
    await waitFor(() => expect(within(rowOf('Automation')).getByTestId('category-hidden-badge')).toBeTruthy())
    expect(within(rowOf('Films')).queryByTestId('category-hidden-badge')).toBeNull()
  })

  it('cannot hide the default category: on and locked, with the reason as its description', async () => {
    mount([category('cat-1', 'Films', { is_default: true }), category('cat-2', 'Automation', { selectable: false })])

    await fireEvent.click(within(rowOf('Automation')).getByRole('button', { name: common.actions.edit }))
    expect(selectableSwitch().getAttribute('aria-checked')).toBe('false')
    // Made the default, it is offered again.
    await fireEvent.click(screen.getByRole('switch', { name: routing.category.default_label }))
    await waitFor(() => expect(selectableSwitch().getAttribute('aria-checked')).toBe('true'))
    expect(selectableSwitch().hasAttribute('disabled')).toBe(true)
    // The stubbed field keeps its description as an attribute.
    const field = document.querySelector('[data-settings-anchor="routing.category_selectable"]')
    expect(field?.getAttribute('description')).toBe(routing.category.selectable_default_hint)

    await fireEvent.click(within(rowOf('Films')).getByRole('button', { name: common.actions.edit }))
    expect(selectableSwitch().getAttribute('aria-checked')).toBe('true')
    expect(selectableSwitch().hasAttribute('disabled')).toBe(true)
  })
})
