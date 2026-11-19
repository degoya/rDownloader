/**
 * The category list in a phone's column (RD-1430-03).
 *
 * At 390 px the list grouped by storage root was 531 px wide in a 358 px card, and the flat rows
 * ran 7 px past it: the badges and *Duplicate*, *Edit*, *Delete* were cut off. A grid track grows
 * to the smallest width of what it holds unless that may shrink, so the rows and the accordion
 * carry `min-w-0`; inside, the row measures itself, as an account row does (`design.md`, *Row and
 * List Conventions*). jsdom lays nothing out, so these tests pin the structure; the widths were
 * measured in a browser at 390 and 1440 px.
 */
import { screen, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock, confirmMock, eventStreamMock } from '@/test/mocks'

import type { Category, StorageRoot } from '@/api/types'
import common from '@/locales/en/common.json'
import routing from '@/locales/en/routing.json'
import { mountComponent, uiStubs } from '@/test/mount'

const get = vi.fn()
vi.mock('@/api/client', () => apiClientMock({
  GET: (...args: unknown[]) => get(...args),
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
const SECOND_ROOT: StorageRoot = { ...ROOT, id: 'root-2', name: 'Archive', path: '/mnt/archive', is_default: false }

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

const CATEGORIES = [
  category('cat-1', 'Films', { is_default: true }),
  category('cat-2', 'Series', { selectable: false })
]

/**
 * The shared stubs drop the `ui` prop; these two show the slot classes the view hands Nuxt UI, as
 * `data-ui-label`, so a test can read what the real component would put on its label.
 */
const stubs = {
  UInputTags: true,
  UButton: {
    props: ['label', 'disabled', 'loading', 'ariaLabel', 'ui'],
    template: '<button type="button" v-bind="$attrs" :disabled="disabled" :aria-label="ariaLabel" :data-ui-label="ui?.label">{{ label }}<slot /></button>'
  },
  UAccordion: {
    ...uiStubs.UAccordion,
    props: [...uiStubs.UAccordion.props, 'ui'],
    template: uiStubs.UAccordion.template.replace('<div v-bind="$attrs">', '<div v-bind="$attrs" :data-ui-label="ui?.label">')
  }
}

function mount(categories: Category[], roots: StorageRoot[]) {
  return mountComponent(RoutingCategories, {
    messages: { routing },
    props: { modelValue: categories, roots, loading: false, loadError: null },
    stubs
  })
}

function rows(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>('[data-category-row]')]
}

/** Every row: shrinks to its list, measures itself, keeps its actions whole and lets its badges wrap. */
function expectNarrowRows(): void {
  expect(rows()).toHaveLength(2)
  for (const row of rows()) {
    expect(row.classList).toContain('min-w-0')
    expect(row.classList).toContain('@container')
    const title = row.querySelector('[data-category-title]') as HTMLElement
    expect(title.classList).toContain('min-w-0')
    expect(title.classList).toContain('flex-wrap')
    const actions = row.querySelector('[data-category-actions]') as HTMLElement
    expect(actions.classList).toContain('shrink-0')
    const buttons = within(actions).getAllByRole('button')
    expect(buttons.map(button => button.getAttribute('aria-label'))).toEqual([
      common.actions.duplicate,
      common.actions.edit,
      common.actions.delete
    ])
    // Below `@sm` *Duplicate* keeps its icon; its name stays in `aria-label` and the hint in `title`.
    const duplicate = buttons[0] as HTMLElement
    expect(duplicate.getAttribute('title')).toBe(common.duplicate_hint)
    expect(duplicate.textContent).toBe(common.actions.duplicate)
    expect(duplicate.dataset.uiLabel).toBe('@max-sm:hidden')
  }
  // The status badges stand beside the name, where they wrap with it, not among the actions.
  const [films, series] = rows() as [HTMLElement, HTMLElement]
  expect(within(films.querySelector('[data-category-title]') as HTMLElement).getByText(routing.category.default_badge)).toBeTruthy()
  expect(within(series.querySelector('[data-category-title]') as HTMLElement).getByTestId('category-hidden-badge')).toBeTruthy()
}

describe('RoutingCategories at 390 px', () => {
  beforeEach(() => {
    get.mockReset()
    get.mockImplementation(async (path: string) =>
      path === '/api/v1/postprocess/scripts' ? { data: { scripts: [], directory: '/scripts' } } : { data: [] }
    )
  })

  it('keeps the flat rows inside the card', () => {
    mount(CATEGORIES, [ROOT])

    expect(screen.queryByTestId('category-groups')).toBeNull()
    expectNarrowRows()
  })

  it('keeps the grouped list and its headers inside the card', () => {
    mount([CATEGORIES[0] as Category, { ...(CATEGORIES[1] as Category), storage_root_id: SECOND_ROOT.id }], [ROOT, SECOND_ROOT])

    const groups = screen.getByTestId('category-groups')
    expect(groups.classList).toContain('min-w-0')
    // The header's label may shrink, so the root's name and path are cut and the badges stay.
    expect(groups.dataset.uiLabel).toBe('min-w-0')
    const headers = [...groups.querySelectorAll<HTMLElement>('[data-accordion-item] > button')]
    expect(headers).toHaveLength(2)
    for (const header of headers) {
      expect(within(header).getByText(/1 category/).classList).toContain('shrink-0')
    }
    expect(within(headers[0] as HTMLElement).getByText(routing.category.default_badge).classList).toContain('shrink-0')
    // The path gives way before the root's name does.
    expect(within(headers[1] as HTMLElement).getByText(SECOND_ROOT.path).classList).toContain('shrink-4')
    expectNarrowRows()
  })
})
