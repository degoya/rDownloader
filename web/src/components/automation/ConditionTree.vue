<script setup lang="ts">
/**
 * Recursive editor for a condition tree.
 *
 * Nesting is shown by indentation and a labelled group rather than by a canvas: a person
 * reading a rule needs to see "all of these, and one of those" in reading order, and a graph
 * makes that harder, not easier — as well as being unusable with a keyboard.
 *
 * The component edits a copy and emits a whole node on every change, so the parent owns the
 * tree and undo stays a matter of not saving.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AutomationCondition, AutomationVocabulary } from '@/api/types'

const props = defineProps<{
  modelValue: AutomationCondition
  vocabulary: AutomationVocabulary | null
  depth: number
  /** Field names that hold a number, so the operator list can narrow itself. */
  numericFields: string[]
}>()
const emit = defineEmits<{
  'update:modelValue': [value: AutomationCondition]
  remove: []
}>()

const { t } = useI18n()

const NUMERIC_OPERATORS = ['greater_than', 'less_than']

const node = computed(() => props.modelValue)
const isGroup = computed(() => node.value.type === 'all' || node.value.type === 'any')
/**
 * The comparison of this node, or null when it is not one.
 *
 * Narrowed here rather than in the template: the generated union does not narrow through a
 * `v-if` in a way the template compiler can follow, and casting at each use would hide a
 * real shape change behind an `as`.
 */
const predicate = computed(() =>
  node.value.type === 'predicate' ? node.value.predicate : null
)
/** Children of a group node, empty for anything else. */
const children = computed<AutomationCondition[]>(() =>
  node.value.type === 'all' || node.value.type === 'any' ? node.value.nodes : []
)
/** The single child of a `not`, or null. */
const negated = computed<AutomationCondition | null>(() =>
  node.value.type === 'not' ? node.value.node : null
)
const atMaxDepth = computed(
  () => props.depth >= (props.vocabulary?.max_condition_depth ?? 6) - 1
)

const typeOptions = computed(() => [
  { value: 'always', label: t('automation.condition.always') },
  { value: 'predicate', label: t('automation.condition.predicate') },
  { value: 'all', label: t('automation.condition.all') },
  { value: 'any', label: t('automation.condition.any') },
  { value: 'not', label: t('automation.condition.not') }
])

const fieldOptions = computed(() =>
  (props.vocabulary?.fields ?? []).map(field => ({
    value: field,
    label: t(`automation.field.${field}`)
  }))
)

/**
 * Operators offered for the field currently chosen.
 *
 * A size compared with `contains`, or a name with `>`, is refused by the server. Filtering
 * here means the author never builds one rather than being told afterwards.
 */
const operatorOptions = computed(() => {
  const field = predicate.value?.field ?? ''
  const numeric = props.numericFields.includes(field)
  return (props.vocabulary?.operators ?? [])
    .filter(operator => NUMERIC_OPERATORS.includes(operator) === numeric)
    .map(operator => ({ value: operator, label: t(`automation.operator.${operator}`) }))
})

function changeType(type: string): void {
  switch (type) {
    case 'always':
      emit('update:modelValue', { type: 'always' } as AutomationCondition)
      break
    case 'predicate':
      emit('update:modelValue', {
        type: 'predicate',
        predicate: { field: 'name', operator: 'contains', value: '' }
      } as AutomationCondition)
      break
    case 'not':
      emit('update:modelValue', {
        type: 'not',
        node: { type: 'always' }
      } as AutomationCondition)
      break
    default:
      emit('update:modelValue', { type, nodes: [{ type: 'always' }] } as AutomationCondition)
  }
}

function updatePredicate(key: 'field' | 'operator' | 'value', value: string): void {
  const current = predicate.value
  if (!current) return
  const next = { ...current, [key]: value } as Record<string, string>
  // Switching to or from a numeric field invalidates the operator; move it to one that fits
  // instead of leaving a combination the server will refuse.
  if (key === 'field') {
    const numeric = props.numericFields.includes(value)
    if (NUMERIC_OPERATORS.includes(next.operator ?? '') !== numeric) {
      next.operator = numeric ? 'greater_than' : 'contains'
    }
  }
  emit('update:modelValue', { type: 'predicate', predicate: next } as AutomationCondition)
}

function updateChild(index: number, value: AutomationCondition): void {
  if (!isGroup.value) return
  const nodes = [...children.value]
  nodes[index] = value
  emit('update:modelValue', { type: node.value.type, nodes } as AutomationCondition)
}

function addChild(): void {
  if (!isGroup.value) return
  const nodes = [...children.value, { type: 'always' } as AutomationCondition]
  emit('update:modelValue', { type: node.value.type, nodes } as AutomationCondition)
}

function removeChild(index: number): void {
  if (!isGroup.value) return
  const nodes = children.value.filter((_, position) => position !== index)
  // A group with nothing in it means "always" or "never" depending on the group, which is
  // never what someone meant to write; the server refuses it and so does this.
  emit('update:modelValue', {
    type: node.value.type,
    nodes: nodes.length ? nodes : [{ type: 'always' }]
  } as AutomationCondition)
}
</script>

<template>
  <div class="border border-muted p-3" :class="depth > 0 ? 'mt-2' : ''">
    <div class="flex flex-wrap items-center gap-2">
      <USelectMenu
        :model-value="node.type"
        :items="typeOptions"
        value-key="value"
        :aria-label="t('automation.condition.kind')"
        class="w-44"
        @update:model-value="(value: string) => changeType(value)"
      />
      <template v-if="predicate">
        <USelectMenu
          :model-value="predicate.field"
          :items="fieldOptions"
          value-key="value"
          :aria-label="t('automation.condition.field')"
          class="w-40"
          @update:model-value="(value: string) => updatePredicate('field', value)"
        />
        <USelectMenu
          :model-value="predicate.operator"
          :items="operatorOptions"
          value-key="value"
          :aria-label="t('automation.condition.operator')"
          class="w-40"
          @update:model-value="(value: string) => updatePredicate('operator', value)"
        />
        <UInput
          :model-value="predicate.value"
          :aria-label="t('automation.condition.value')"
          :placeholder="t('automation.condition.value')"
          class="w-48"
          @update:model-value="(value: string | number) => updatePredicate('value', String(value))"
        />
      </template>
      <div class="ml-auto flex gap-1">
        <UButton
          v-if="isGroup && !atMaxDepth"
          icon="i-lucide-plus"
          size="xs"
          color="neutral"
          variant="ghost"
          :label="t('automation.condition.add')"
          @click="addChild"
        />
        <UButton
          v-if="depth > 0"
          icon="i-lucide-x"
          size="xs"
          color="error"
          variant="ghost"
          :aria-label="t('automation.condition.remove')"
          @click="emit('remove')"
        />
      </div>
    </div>

    <div v-if="negated" class="ml-4">
      <ConditionTree
        :model-value="negated"
        :vocabulary="vocabulary"
        :depth="depth + 1"
        :numeric-fields="numericFields"
        @update:model-value="(value: AutomationCondition) => emit('update:modelValue', { type: 'not', node: value } as AutomationCondition)"
      />
    </div>
    <div v-else-if="isGroup" class="ml-4">
      <ConditionTree
        v-for="(child, index) in children"
        :key="index"
        :model-value="child"
        :vocabulary="vocabulary"
        :depth="depth + 1"
        :numeric-fields="numericFields"
        @update:model-value="(value: AutomationCondition) => updateChild(index, value)"
        @remove="removeChild(index)"
      />
    </div>
  </div>
</template>
