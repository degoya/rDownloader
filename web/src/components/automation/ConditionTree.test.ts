import { render, screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from 'vue-i18n'

import type { AutomationCondition, AutomationVocabulary } from '@/api/types'
import en from '@/locales/en/automation.json'

import ConditionTree from './ConditionTree.vue'

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { automation: en } } })

const vocabulary = {
  triggers: [],
  fields: ['name', 'domain', 'size_bytes'],
  operators: [
    'equals',
    'contains',
    'starts_with',
    'ends_with',
    'matches',
    'greater_than',
    'less_than'
  ],
  action_kinds: ['pause_package'],
  max_actions: 10,
  max_condition_depth: 6
} as unknown as AutomationVocabulary

/** Records what the component emits, which is the whole node every time. */
function mount(modelValue: AutomationCondition, depth = 0) {
  // Named `changes` rather than `emitted`: testing-library's render result already has an
  // `emitted` helper, and spreading it would shadow this one.
  const changes: AutomationCondition[] = []
  const utils = render(ConditionTree, {
    props: {
      modelValue,
      vocabulary,
      depth,
      numericFields: ['size_bytes'],
      'onUpdate:modelValue': (value: AutomationCondition) => changes.push(value)
    },
    global: {
      plugins: [i18n],
      stubs: {
        USelectMenu: {
          props: ['modelValue', 'items'],
          emits: ['update:modelValue'],
          template:
            '<select v-bind="$attrs" @change="$emit(\'update:modelValue\', items[$event.target.selectedIndex])">' +
            '<option v-for="item in items" :key="item.value ?? item.id">{{ item.label ?? item.name }}</option></select>'
        },
        UInput: {
          props: ['modelValue'],
          emits: ['update:modelValue'],
          template:
            '<input v-bind="$attrs" :value="modelValue" @input="$emit(\'update:modelValue\', $event.target.value)" />'
        },
        UButton: {
          props: ['label'],
          template: '<button v-bind="$attrs">{{ label }}</button>'
        }
      }
    }
  })
  return { changes, ...utils }
}

describe('ConditionTree', () => {
  it('renders a comparison with its field, operator and value', () => {
    mount({
      type: 'predicate',
      predicate: { field: 'name', operator: 'contains', value: 'S01E01' }
    } as AutomationCondition)
    expect(screen.getByLabelText('Field')).toBeTruthy()
    expect(screen.getByLabelText('Operator')).toBeTruthy()
    expect((screen.getByLabelText('Value') as HTMLInputElement).value).toBe('S01E01')
  })

  it('offers only numeric operators once a numeric field is chosen', () => {
    // A size compared with `contains` is refused by the server; the editor must not be able
    // to build one in the first place.
    mount({
      type: 'predicate',
      predicate: { field: 'size_bytes', operator: 'greater_than', value: '100' }
    } as AutomationCondition)
    const operators = screen.getByLabelText('Operator') as HTMLSelectElement
    const labels = Array.from(operators.options).map(option => option.textContent?.trim())
    expect(labels).toEqual(['greater than', 'less than'])
  })

  it('offers only text operators for a text field', () => {
    mount({
      type: 'predicate',
      predicate: { field: 'name', operator: 'contains', value: '' }
    } as AutomationCondition)
    const operators = screen.getByLabelText('Operator') as HTMLSelectElement
    const labels = Array.from(operators.options).map(option => option.textContent?.trim())
    expect(labels).not.toContain('greater than')
    expect(labels).toContain('contains')
  })

  it('renders nested groups so a tree is readable in reading order', () => {
    const { container } = mount({
      type: 'all',
      nodes: [
        { type: 'predicate', predicate: { field: 'name', operator: 'contains', value: 'a' } },
        {
          type: 'any',
          nodes: [
            { type: 'predicate', predicate: { field: 'domain', operator: 'equals', value: 'b' } }
          ]
        }
      ]
    } as AutomationCondition)
    // Three comparison rows in total: two predicates plus the group headers around them.
    expect(container.querySelectorAll('input').length).toBe(2)
  })

  it('keeps a group non-empty, because an empty one silently means always or never', async () => {
    const { changes, container } = mount({
      type: 'all',
      nodes: [{ type: 'predicate', predicate: { field: 'name', operator: 'contains', value: 'a' } }]
    } as AutomationCondition)
    const remove = Array.from(container.querySelectorAll('button')).find(
      button => button.getAttribute('aria-label') === 'Remove condition'
    )
    expect(remove).toBeTruthy()
    remove?.click()
    await vi.waitFor(() => expect(changes.length).toBe(1))
    const next = changes[0] as { type: string; nodes: AutomationCondition[] }
    expect(next.type).toBe('all')
    expect(next.nodes).toEqual([{ type: 'always' }])
  })

  it('round-trips an unchanged tree through the model', async () => {
    const tree = {
      type: 'not',
      node: {
        type: 'any',
        nodes: [
          { type: 'predicate', predicate: { field: 'name', operator: 'ends_with', value: '.mkv' } }
        ]
      }
    } as AutomationCondition
    const { changes } = mount(tree)
    // Rendering alone must not rewrite the tree: an editor that normalises on open turns
    // "look at this rule" into "change this rule".
    expect(changes).toEqual([])
  })
})
