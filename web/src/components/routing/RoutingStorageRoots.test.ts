import { render, screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from 'vue-i18n'

import type { StorageRoot } from '@/api/types'
import routing from '@/locales/en/routing.json'
import common from '@/locales/en/common.json'

import RoutingStorageRoots from './RoutingStorageRoots.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'rejected'),
  resultMessage: vi.fn(() => 'Storage root deleted')
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { routing, common } } })

const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }
const components = {
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
  USwitch: {
    props: ['modelValue', 'disabled'],
    emits: ['update:modelValue'],
    template:
      '<input type="checkbox" role="switch" v-bind="$attrs" :disabled="disabled" :checked="modelValue" @change="$emit(\'update:modelValue\', $event.target.checked)" />'
  },
  UAlert: {
    props: ['title', 'description', 'color'],
    template: '<div :data-color="color">{{ title }} {{ description }}</div>'
  },
  UFormField: passthrough,
  UBadge: passthrough,
  UIcon: { template: '<span />' }
}

function root(overrides: Partial<StorageRoot> = {}): StorageRoot {
  return {
    id: 'root-1',
    name: 'Downloads',
    path: '/downloads',
    is_default: true,
    minimum_free_bytes: null,
    persistence: 'persistent',
    ...overrides
  }
}

function mount(roots: StorageRoot[]) {
  return render(RoutingStorageRoots, {
    props: { modelValue: roots },
    global: { plugins: [i18n], components }
  })
}

describe('RoutingStorageRoots', () => {
  it('flags a root whose path will not survive the container', () => {
    mount([root({ persistence: 'ephemeral' })])

    expect(screen.getByText(routing.root.ephemeral_badge)).toBeTruthy()
    expect(screen.getByText(new RegExp(routing.root.ephemeral_title))).toBeTruthy()
  })

  it('says nothing when every root is on persistent storage', () => {
    mount([root(), root({ id: 'root-2', name: 'Movies', path: '/movies', is_default: false })])

    expect(screen.queryByText(routing.root.ephemeral_badge)).toBeNull()
    expect(screen.queryByText(new RegExp(routing.root.ephemeral_title))).toBeNull()
  })

  it('treats an unknown verdict as no news rather than bad news', () => {
    mount([root({ persistence: 'unknown' })])

    expect(screen.queryByText(routing.root.ephemeral_badge)).toBeNull()
  })

  it('locks the default switch while there is no root to hand it to', () => {
    mount([])

    const toggle = screen.getByRole('switch') as HTMLInputElement
    expect(toggle.disabled).toBe(true)
  })

  it('leaves the default switch usable once a second root can take over', () => {
    mount([root(), root({ id: 'root-2', name: 'Movies', path: '/movies', is_default: false })])

    const toggle = screen.getByRole('switch') as HTMLInputElement
    expect(toggle.disabled).toBe(false)
  })
})
