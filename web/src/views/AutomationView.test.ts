/**
 * The reported case (RD-106-16).
 *
 * "The webhook dropdown in an automation is empty although I created two webhooks." Both
 * targets arrived; the select read each item's label from a field the target DTO does not
 * have, so every row rendered as an empty line and the search box, filtering on the same
 * field, hid them entirely. A select that names its options by the wrong key looks exactly
 * like one with nothing to offer.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import automation from '@/locales/en/automation.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: vi.fn(),
    PUT: vi.fn(),
    PATCH: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: vi.fn(() => 'The service did not answer'),
  resultMessage: vi.fn(() => '')
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))
// The view subscribes to `automation.changed` on mount, and jsdom has no `EventSource`. These
// tests are about the form, so the stream is stubbed away; the subscription itself is covered
// in `stores/automations.test.ts`.
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
// The backup buttons import a toast through the Nuxt UI barrel, which needs a Nuxt build.
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: vi.fn() }) })
}))

const { default: AutomationView } = await import('./AutomationView.vue')

const VOCABULARY = {
  triggers: ['download_completed'],
  fields: ['name'],
  operators: ['equals'],
  action_kinds: ['pause_package', 'webhook'],
  max_actions: 10,
  max_condition_depth: 3
}

const TARGETS = [
  { id: 'target-1', name: 'Ops hook', kind: 'webhook', enabled: true, endpoint: 'https://ops.example/hook', config: {}, has_secret: false },
  { id: 'target-2', name: 'Backup hook', kind: 'webhook', enabled: true, endpoint: 'https://backup.example/hook', config: {}, has_secret: false }
]

/**
 * `USelectMenu` rendered as a plain `<select>` that names each option the way the real
 * component does: through `label-key`, or `label` when none is given. That is the whole point
 * of this test — a stub that printed `item.name` regardless would pass with the defect in place.
 */
const selectMenu = {
  props: ['modelValue', 'items', 'labelKey', 'valueKey'],
  emits: ['update:modelValue'],
  template: `
    <select v-bind="$attrs" :value="modelValue" @change="$emit('update:modelValue', $event.target.value)">
      <option v-for="item in items" :key="valueKey ? item[valueKey] : item.value" :value="valueKey ? item[valueKey] : item.value">
        {{ labelKey ? item[labelKey] : item.label }}
      </option>
    </select>`
}

function mount() {
  return mountComponent(AutomationView, {
    messages: { automation },
    stubs: {
      USelectMenu: selectMenu,
      // The shared stub renders only the default slot; the create button sits in `#right`.
      UDashboardNavbar: { template: '<div><slot /><slot name="right" /></div>' },
      ConditionTree: true,
      AreaBackupButtons: true
    }
  })
}

describe('AutomationView', () => {
  beforeEach(() => {
    get.mockReset()
    get.mockImplementation(async (path: string) => {
      if (path === '/api/v1/automations/vocabulary') return { data: VOCABULARY }
      if (path === '/api/v1/notifications/targets') return { data: TARGETS }
      return { data: [] }
    })
  })

  it('offers every notification target by its name for a webhook action', async () => {
    mount()
    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/notifications/targets'))

    // Two ways in — the navbar and the empty form column — and either opens the editor.
    await fireEvent.click(screen.getAllByRole('button', { name: automation.create })[0]!)
    const kind = await screen.findByLabelText(automation.action.kind)
    await fireEvent.update(kind, 'webhook')

    const target = await screen.findByLabelText(automation.action.target)
    const names = Array.from(target.querySelectorAll('option')).map(option => option.textContent?.trim())
    expect(names).toEqual(['Ops hook', 'Backup hook'])
    expect(screen.queryByText(automation.action.no_targets)).toBeNull()
  })

  it('keeps the form heading above both the empty and open cards', async () => {
    mount()
    const heading = await screen.findByRole('heading', { name: automation.create_title })

    expect(heading.nextElementSibling?.className).toContain('border-dashed')
    await fireEvent.click(screen.getAllByRole('button', { name: automation.create })[0]!)
    expect(heading.nextElementSibling?.className).toContain('bg-default')
    expect(heading.nextElementSibling?.className).not.toContain('border-dashed')
  })
})
