import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import en from '@/locales/en/automation.json'
import type { DraftAction } from '@/composables/useAutomationDraft'
import { mountComponent } from '@/test/mount'

import AutomationActionCard from './AutomationActionCard.vue'

const select = {
  props: ['modelValue', 'items', 'valueKey', 'labelKey'],
  emits: ['update:modelValue'],
  template:
    '<select v-bind="$attrs" :value="modelValue" @change="$emit(\'update:modelValue\', $event.target.value)">' +
    '<option v-for="item in items" :key="valueKey ? item[valueKey] : item.value" :value="valueKey ? item[valueKey] : item.value">' +
    '{{ labelKey ? item[labelKey] : item.label }}</option></select>'
}

const TARGETS = [
  { id: 'target-1', name: 'Ops hook', kind: 'webhook', enabled: true, endpoint: 'https://ops.example/hook', config: {}, has_secret: false }
]

function mount(action: DraftAction) {
  const changes: DraftAction[] = []
  mountComponent(AutomationActionCard, {
    messages: { automation: en },
    props: {
      modelValue: action,
      kindOptions: [{ value: action.kind, label: action.kind }],
      scriptItems: [],
      categories: [],
      targets: TARGETS,
      'onUpdate:modelValue': (value: DraftAction) => changes.push(value)
    },
    stubs: { USelectMenu: select, USelect: select }
  })
  return { changes }
}

describe('AutomationActionCard (RD-1240-10)', () => {
  it('edits a notification: its target and its message', async () => {
    const { changes } = mount({ kind: 'notify', target_id: 'target-1', message: '' })
    expect(screen.getByText('Ops hook')).toBeTruthy()
    await fireEvent.update(screen.getByLabelText(en.action.message), 'Night queue started')
    expect(changes.at(-1)).toEqual({ kind: 'notify', target_id: 'target-1', message: 'Night queue started' })
  })

  it('edits links as text and their destination', async () => {
    const { changes } = mount({ kind: 'add_links', links_text: '', destination: 'link_grabber' })
    // The text area is a one-line stub here; the lines are `linksOf`'s, tested beside the draft.
    await fireEvent.update(screen.getByLabelText(en.action.links), 'https://a.example/1')
    expect(changes.at(-1)?.links_text).toBe('https://a.example/1')
    await fireEvent.update(screen.getByLabelText(en.action.destination), 'downloads')
    expect(changes.at(-1)?.destination).toBe('downloads')
  })

  it('offers the three priorities', () => {
    mount({ kind: 'set_priority', priority: 'high' })
    const options = Array.from((screen.getByLabelText(en.action.priority) as HTMLSelectElement).options).map(option => option.textContent?.trim())
    expect(options).toEqual([en.action.priority_value.low, en.action.priority_value.normal, en.action.priority_value.high])
  })
})
