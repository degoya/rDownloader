import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import type { StorageRoot } from '@/api/types'
import routing from '@/locales/en/routing.json'
import { mountComponent } from '@/test/mount'

import RoutingStorageRoots from './RoutingStorageRoots.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'rejected'),
  resultMessage: vi.fn(() => 'Storage root deleted')
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))

/** The field with its error under the control, where the protected-path refusal is read. */
const UFormField = {
  props: ['error'],
  template: '<div><slot /><p v-if="error" data-testid="field-error">{{ error }}</p></div>'
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

function mount(roots: StorageRoot[], props: Record<string, unknown> = {}) {
  return mountComponent(RoutingStorageRoots, {
    messages: { routing },
    stubs: { UFormField },
    props: { modelValue: roots, ...props }
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

  it('offers the suggested name for the first root, so the required field is not empty', () => {
    mount([], { suggestedName: 'Downloads', suggestedPath: '/srv/downloads' })

    expect((screen.getByPlaceholderText(routing.root.name_placeholder) as HTMLInputElement).value).toBe('Downloads')
  })

  it('shows a path refusal under the path field and says what to pick for a protected one', async () => {
    vi.mocked(api.POST).mockResolvedValueOnce({
      error: { error: 'protected', code: 'storage_root.protected_directory', params: { directory: '/data' } }
    } as never)
    const { container } = mount([], { suggestedName: 'Downloads', suggestedPath: '/data/downloads' })

    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)

    await waitFor(() => expect(screen.getByTestId('field-error').textContent).toContain(routing.root.protected_hint))
    expect(document.activeElement?.getAttribute('name')).toBe('path')
  })

  it('keeps a refusal that is not about the path above the form', async () => {
    const scrolled = vi.fn()
    Element.prototype.scrollIntoView = scrolled
    vi.mocked(api.POST).mockResolvedValueOnce({ error: { error: 'busy', code: 'storage_root.in_use' } } as never)
    const { container } = mount([], { suggestedName: 'Downloads', suggestedPath: '/srv/downloads' })

    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)

    await waitFor(() => expect(screen.getByText(/rejected/)).toBeTruthy())
    expect(screen.queryByTestId('field-error')).toBeNull()
    expect(scrolled).toHaveBeenCalled()
  })
})
