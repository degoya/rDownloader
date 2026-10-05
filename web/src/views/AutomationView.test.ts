/**
 * The reported case (RD-106-16).
 *
 * "The webhook dropdown in an automation is empty although I created two webhooks." Both
 * targets arrived; the select read each item's label from a field the target DTO does not
 * have, so every row rendered as an empty line and the search box, filtering on the same
 * field, hid them entirely. A select that names its options by the wrong key looks exactly
 * like one with nothing to offer.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import common from '@/locales/en/common.json'
import automation from '@/locales/en/automation.json'
import { mountComponent } from '@/test/mount'
import { axeViolations } from '@/test/axe'

const get = vi.fn()
const post = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
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
      ConditionTree: true,
      AreaBackupButtons: true
    }
  })
}

describe('AutomationView', () => {
  beforeEach(() => {
    get.mockReset()
    post.mockReset()
    get.mockImplementation(async (path: string) => {
      if (path === '/api/v1/automations/vocabulary') return { data: VOCABULARY }
      if (path === '/api/v1/notifications/targets') return { data: TARGETS }
      return { data: [] }
    })
  })

  it('offers every notification target by its name for a webhook action', async () => {
    mount()
    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/notifications/targets'))

    // One way in: the empty form column offers it (RD-150-11 removed the second in the navbar).
    expect(screen.getAllByRole('button', { name: automation.create })).toHaveLength(1)
    await fireEvent.click(screen.getByRole('button', { name: automation.create }))
    const kind = await screen.findByLabelText(automation.action.kind)
    await fireEvent.update(kind, 'webhook')

    const target = await screen.findByLabelText(automation.action.target)
    const names = Array.from(target.querySelectorAll('option')).map(option => option.textContent?.trim())
    expect(names).toEqual(['Ops hook', 'Backup hook'])
    expect(screen.queryByText(automation.action.no_targets)).toBeNull()
  })

  it('keeps the form heading in one card with the empty prompt and the open form', async () => {
    mount()
    const heading = (await screen.findByRole('heading', { name: automation.create_title })).parentElement as HTMLElement
    const card = heading.closest('section') as HTMLElement

    // The column's card, not a dashed box beside the heading (RD-1110-17).
    expect(card.contains(screen.getByText(automation.pick_or_create))).toBe(true)
    expect(card.querySelector('.border-dashed')).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: automation.create }))
    expect(card.contains(screen.getByTestId('automation-form'))).toBe(true)
    expect(heading.nextElementSibling).toBe(screen.getByTestId('automation-form'))
  })

  it('asks for the trigger first and ends the form with one action row', async () => {
    mount()
    await fireEvent.click(await screen.findByRole('button', { name: automation.create }))
    const form = await screen.findByTestId('automation-form')
    expect(form.tagName).toBe('FORM')
    // The trigger is the automation's kind; the first field the form asks for.
    expect(form.querySelector('label')?.textContent).toContain(automation.trigger_label)
    // Primary action first, no cancel while creating, and the Active switch is a field, not a
    // member of the row.
    const row = form.querySelector('[data-form-actions]') as HTMLElement
    const buttons = within(row).getAllByRole('button')
    expect(buttons.map(button => button.textContent)).toEqual([automation.create_title])
    expect(buttons[0]?.getAttribute('type')).toBe('submit')
    expect(within(row).queryByRole('switch')).toBeNull()
  })

  it('duplicates an automation switched off, under a new name, and opens the copy for editing', async () => {
    const original = {
      id: 'a1', name: 'Pause big', enabled: true, version: 3, created_at: '', updated_at: '',
      definition: { trigger: 'download_completed', condition: { type: 'always' }, actions: [{ kind: 'pause_package' }] }
    }
    const copy = { ...original, id: 'a2', name: `Pause big (${common.copy_suffix})`, enabled: false, version: 1 }
    let listed: unknown[] = [original]
    get.mockImplementation(async (path: string) => {
      if (path === '/api/v1/automations/vocabulary') return { data: VOCABULARY }
      if (path === '/api/v1/notifications/targets') return { data: TARGETS }
      if (path === '/api/v1/automations') return { data: listed }
      return { data: [] }
    })
    post.mockImplementation(async () => {
      listed = [original, copy]
      return { data: copy }
    })
    mount()

    await fireEvent.click(await screen.findByRole('button', { name: common.actions.duplicate }))

    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/automations', expect.anything()))
    const body = post.mock.calls[0]?.[1]?.body
    expect(body).toEqual({
      name: `Pause big (${common.copy_suffix})`,
      enabled: false,
      trigger: 'download_completed',
      condition: { type: 'always' },
      actions: [{ kind: 'pause_package' }]
    })
    await screen.findByRole('heading', { name: automation.edit_title })
    expect((screen.getByDisplayValue(copy.name) as HTMLInputElement).value).toBe(copy.name)
    expect(screen.getByText(common.editing)).toBeTruthy()
  })

  it('renders without an axe violation', async () => {
    const { container } = mount()
    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/notifications/targets'))
    expect(await axeViolations(container)).toBe('')
  })

})
