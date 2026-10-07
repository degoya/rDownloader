/**
 * RD-1140-05: a category's override of the package-name rules, each switch inherit / on / off,
 * and the preview of what a new package of the category gets — the override over the saved
 * global switches, which the service fills in for every switch left at "inherit".
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { PackageNameRulesOverride } from '@/api/types'
import routing from '@/locales/en/routing.json'
import settings from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'
import { INHERIT_LEVEL } from '@/utils/format'

const post = vi.fn()
vi.mock('@/api/client', () => ({ api: { POST: (...args: unknown[]) => post(...args) } }))

const { default: RoutingCategoryPackageNames } = await import('./RoutingCategoryPackageNames.vue')

/** A native select carrying the item values, so a test picks one with a change event. */
const USelect = {
  props: ['modelValue', 'items'],
  emits: ['update:modelValue'],
  template: '<select v-bind="$attrs" :value="modelValue" @change="$emit(\'update:modelValue\', $event.target.value)">'
    + '<option v-for="item in items" :key="item.value" :value="item.value">{{ item.label }}</option></select>'
}

describe('RoutingCategoryPackageNames', () => {
  beforeEach(() => {
    post.mockReset()
    post.mockResolvedValue({ data: { name: 'big.buck.bunny.[1080p]', folder: 'big.buck.bunny.[1080p]', rules: {} } })
  })

  it('sets one switch of the override and leaves the others inheriting', async () => {
    const updates: (PackageNameRulesOverride | null)[] = []
    mountComponent(RoutingCategoryPackageNames, {
      messages: { routing, settings },
      props: { 'modelValue': null, 'onUpdate:modelValue': (value: PackageNameRulesOverride | null) => updates.push(value) },
      stubs: { USelect }
    })
    const select = screen.getByTestId('category-package-names-lowercase') as HTMLSelectElement
    expect(select.value).toBe(INHERIT_LEVEL)
    await fireEvent.update(select, 'on')

    expect(updates.at(-1)).toEqual({ spaces_to_dots: null, collapse_separators: null, strip_bracket_tags: null, lowercase: true })
  })

  it('previews the override over the saved global switches', async () => {
    mountComponent(RoutingCategoryPackageNames, {
      messages: { routing, settings },
      props: { modelValue: { lowercase: true } },
      stubs: { USelect }
    })

    await waitFor(() => expect(screen.getByTestId('category-package-names-preview').textContent).toContain('big.buck.bunny'))
    const request = post.mock.calls[0]?.[1] as { body: { rules: Record<string, boolean | null>, regex: unknown } }
    expect(request.body.rules).toEqual({ spaces_to_dots: null, collapse_separators: null, strip_bracket_tags: null, lowercase: true })
    // No list of its own: the saved global pairs apply.
    expect(request.body.regex).toBeNull()
  })

  it('switches to a list of its own, which replaces the global one even empty', async () => {
    const updates: unknown[] = []
    mountComponent(RoutingCategoryPackageNames, {
      messages: { routing, settings },
      props: { 'modelValue': null, 'regex': null, 'onUpdate:regex': (value: unknown) => updates.push(value) },
      stubs: { USelect }
    })
    expect(screen.queryByTestId('package-name-regex')).toBeNull()

    await fireEvent.click(screen.getByTestId('category-package-names-regex-own'))

    expect(updates.at(-1)).toEqual([])
  })

  it('previews an empty list of its own as no pairs at all', async () => {
    mountComponent(RoutingCategoryPackageNames, {
      messages: { routing, settings },
      props: { modelValue: null, regex: [] },
      stubs: { USelect }
    })

    await waitFor(() => expect(post).toHaveBeenCalled())
    expect((post.mock.calls[0]?.[1] as { body: { regex: unknown } }).body.regex).toEqual([])
    expect(screen.getByTestId('package-name-regex')).toBeTruthy()
  })
})

describe('RoutingCategoryPackageNames while everything inherits', () => {
  it('asks for no preview', async () => {
    post.mockReset()
    mountComponent(RoutingCategoryPackageNames, {
      messages: { routing, settings },
      props: { modelValue: null },
      stubs: { USelect }
    })
    await new Promise(resolve => setTimeout(resolve, 350))
    expect(post).not.toHaveBeenCalled()
    expect(screen.queryByTestId('category-package-names-preview')).toBeNull()
  })
})
