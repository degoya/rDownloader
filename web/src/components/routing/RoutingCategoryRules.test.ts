/**
 * RD-150-12: a category rule is copied the way every other list copies — the shared copy name,
 * the priority right after the original, and the copy open in the form to be changed.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { Category, CategoryRule } from '@/api/types'
import common from '@/locales/en/common.json'
import routing from '@/locales/en/routing.json'
import { mountComponent } from '@/test/mount'

const post = vi.fn()
const put = vi.fn()
const editRegex = vi.fn()
vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: (...args: unknown[]) => post(...args), PUT: (...args: unknown[]) => put(...args), PATCH: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'rejected'),
  resultMessage: vi.fn(() => '')
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))
vi.mock('@/composables/useRegexEditor', () => ({ useRegexEditor: () => (...args: unknown[]) => editRegex(...args) }))

const { default: RoutingCategoryRules } = await import('./RoutingCategoryRules.vue')

const CATEGORY = { id: 'cat-1', name: 'Movies' } as Category

function rule(id: string, name: string, priority: number): CategoryRule {
  return {
    id,
    name,
    priority,
    category_id: CATEGORY.id,
    enabled: true,
    domain: 'example.com',
    extension: 'mkv',
    protocol: 'https',
    source: 'clipboard',
    mime_type: null,
    name_regex: '^Film'
  } as CategoryRule
}

function rowOf(name: string): HTMLElement {
  return screen.getByText(name).closest('div.flex') as HTMLElement
}

describe('RoutingCategoryRules duplicate', () => {
  beforeEach(() => {
    post.mockReset()
    post.mockImplementation(async (_path: string, { body }: { body: Record<string, unknown> }) => ({ data: { ...body, id: 'rule-copy' } }))
  })

  it('copies the conditions under a free name after the original and opens the copy in the form', async () => {
    mountComponent(RoutingCategoryRules, {
      messages: { routing },
      props: { modelValue: [rule('rule-1', 'Films', 100), rule('rule-2', 'Films (copy)', 101)], categories: [CATEGORY] }
    })

    await fireEvent.click(within(rowOf('Films')).getByRole('button', { name: common.actions.duplicate }))

    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    expect(post.mock.calls[0]?.[1]?.body).toMatchObject({
      name: 'Films (copy 2)',
      priority: 102,
      category_id: CATEGORY.id,
      domain: 'example.com',
      extension: 'mkv',
      protocol: 'https',
      source: 'clipboard',
      name_regex: '^Film',
      // A rule from before RD-1140-02 carries no target; its copy keeps matching the file name.
      name_target: 'file'
    })
    await waitFor(() => expect(screen.getByRole('heading', { level: 3, name: routing.rule.form_edit })).toBeTruthy())
    expect(within(rowOf('Films (copy 2)')).getByText(common.editing)).toBeTruthy()
    expect((screen.getByPlaceholderText(routing.rule.name_placeholder) as HTMLInputElement).value).toBe('Films (copy 2)')
    expect(screen.getByText(routing.rule.duplicated)).toBeTruthy()
  })
})

/** RD-1140-02: the name pattern can be matched against the package name, or either name. */
describe('RoutingCategoryRules name target', () => {
  beforeEach(() => {
    post.mockReset()
    put.mockReset()
    editRegex.mockReset()
    post.mockImplementation(async (_path: string, { body }: { body: Record<string, unknown> }) => ({ data: { ...body, id: 'rule-new' } }))
    put.mockImplementation(async (_path: string, { body }: { body: Record<string, unknown> }) => ({ data: { ...body, id: 'rule-1' } }))
  })

  it('creates a rule whose pattern targets the package name', async () => {
    mountComponent(RoutingCategoryRules, { messages: { routing }, props: { modelValue: [], categories: [CATEGORY] } })

    const name = screen.getByPlaceholderText(routing.rule.name_placeholder) as HTMLInputElement
    await fireEvent.update(name, 'Switch updates')
    await fireEvent.update(screen.getByPlaceholderText(routing.rule.regex_placeholder), '(?i)update.*nsw-')
    const target = screen.getByTestId('rule-name-target') as HTMLSelectElement
    expect(target.value).toBe('file')
    await fireEvent.update(target, 'package')
    await fireEvent.submit(name.closest('form') as HTMLFormElement)

    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    expect(post.mock.calls[0]?.[1]?.body).toMatchObject({ name: 'Switch updates', name_regex: '(?i)update.*nsw-', name_target: 'package' })
  })

  it('names a package target in the list, loads it into the form and hands it to the regex tester', async () => {
    const packageRule = { ...rule('rule-1', 'Films', 100), name_target: 'package' } as CategoryRule
    mountComponent(RoutingCategoryRules, { messages: { routing }, props: { modelValue: [packageRule], categories: [CATEGORY] } })

    expect(within(rowOf('Films')).getByText(new RegExp(`${routing.rule.name_targets.package}: /\\^Film/`))).toBeTruthy()
    await fireEvent.click(within(rowOf('Films')).getByRole('button', { name: common.actions.edit }))
    const target = screen.getByTestId('rule-name-target') as HTMLSelectElement
    await waitFor(() => expect(target.value).toBe('package'))

    await fireEvent.click(screen.getByRole('button', { name: routing.rule.regex_editor.open_button }))
    expect(editRegex).toHaveBeenCalledWith('^Film', 'package')

    await fireEvent.update(target, 'either')
    await fireEvent.submit(target.closest('form') as HTMLFormElement)
    await waitFor(() => expect(put).toHaveBeenCalledTimes(1))
    expect(put.mock.calls[0]?.[1]?.body).toMatchObject({ name_regex: '^Film', name_target: 'either' })
  })
})
