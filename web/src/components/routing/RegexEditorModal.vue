<script setup lang="ts">
import { watchDebounced } from '@vueuse/core'
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { TestRegexResponse, TestRegexSampleResult } from '@/api/types'
import type { RegexConditionKind } from '@/utils/regexBuilder'
import { allowedKinds, buildPattern, parsePattern } from '@/utils/regexBuilder'

const props = defineProps<{ pattern: string | null }>()
const emit = defineEmits<{ close: [result: { pattern: string | null } | null] }>()
const { t } = useI18n()

const EMPTY_CONDITION = { kind: 'contains' as RegexConditionKind, value: '' }
const initial = props.pattern ? parsePattern(props.pattern) : { conditions: [{ ...EMPTY_CONDITION }], caseInsensitive: false }
const pattern = ref(props.pattern ?? '')
const builder = reactive(initial ?? { conditions: [], caseInsensitive: false })
const builderUsable = ref(initial !== null)
const activeTab = ref(initial ? 'visual' : 'expert')
const samples = ref(['Movie.2024.1080p.x265.mkv', 'Show.S01E01.720p.mp4'])

/** Last backend evaluation with the inputs it was computed for, so stale results never render. */
interface Evaluation { pattern: string, samples: string[], response: TestRegexResponse }
const evaluation = ref<Evaluation | null>(null)
let sequence = 0

const current = computed(() => pattern.value.trim())
const upToDate = computed(() => !current.value || evaluation.value?.pattern === current.value)
const invalid = computed(() => Boolean(current.value) && upToDate.value && evaluation.value?.response.valid === false)
const canApply = computed(() => !current.value || (upToDate.value && evaluation.value?.response.valid === true))

const tabItems = computed(() => [
  { value: 'visual', slot: 'visual', label: t('routing.rule.regex_editor.tab_visual'), icon: 'i-lucide-blocks' },
  { value: 'expert', slot: 'expert', label: t('routing.rule.regex_editor.tab_expert'), icon: 'i-lucide-braces' }
])

function kindItems(index: number): { label: string, value: RegexConditionKind }[] {
  return allowedKinds(index, builder.conditions.length)
    .map(kind => ({ label: t(`routing.rule.regex_editor.kind_${kind}`), value: kind }))
}

/** Coerces kinds that became invalid after adding/removing rows back to `contains`. */
function normalizeKinds(): void {
  builder.conditions.forEach((condition, index) => {
    if (!allowedKinds(index, builder.conditions.length).includes(condition.kind)) condition.kind = 'contains'
  })
}

function addCondition(): void {
  builder.conditions.push({ ...EMPTY_CONDITION })
  normalizeKinds()
}

function removeCondition(index: number): void {
  builder.conditions.splice(index, 1)
  if (!builder.conditions.length) builder.conditions.push({ ...EMPTY_CONDITION })
  normalizeKinds()
}

function rebuild(): void {
  builder.conditions = [{ ...EMPTY_CONDITION }]
  builder.caseInsensitive = false
  builderUsable.value = true
  pattern.value = ''
}

watch(builder, () => {
  if (activeTab.value !== 'visual' || !builderUsable.value) return
  const conditions = builder.conditions.filter(condition => condition.value)
  pattern.value = buildPattern({ conditions, caseInsensitive: builder.caseInsensitive })
}, { deep: true })

watch(activeTab, (tab) => {
  if (tab !== 'visual') return
  if (!current.value) return rebuild()
  const state = parsePattern(current.value)
  if (!state) return void (builderUsable.value = false)
  builder.conditions = state.conditions
  builder.caseInsensitive = state.caseInsensitive
  builderUsable.value = true
})

function addSample(): void {
  samples.value.push('')
}

function removeSample(index: number): void {
  samples.value.splice(index, 1)
}

async function evaluate(): Promise<void> {
  const requested = current.value
  if (!requested) return void (evaluation.value = null)
  const id = ++sequence
  const requestedSamples = [...samples.value]
  const response = await api.POST('/api/v1/category-rules/test-regex', {
    body: { pattern: requested, samples: requestedSamples }
  })
  if (id !== sequence || !response.data) return
  evaluation.value = { pattern: requested, samples: requestedSamples, response: response.data }
}

onMounted(() => void evaluate())
watchDebounced([pattern, samples], () => void evaluate(), { debounce: 300, deep: true })

function sampleResult(index: number): TestRegexSampleResult | null {
  const done = evaluation.value
  if (!done || !done.response.valid || done.pattern !== current.value) return null
  if (done.samples[index] !== samples.value[index]) return null
  return done.response.results[index] ?? null
}

function sampleParts(index: number): { before: string, match: string, after: string } | null {
  const result = sampleResult(index)
  if (!result?.matched || result.start == null || result.end == null) return null
  const text = samples.value[index] ?? ''
  return { before: text.slice(0, result.start), match: text.slice(result.start, result.end), after: text.slice(result.end) }
}

function sampleIcon(index: number): { name: string, class: string, label: string } {
  const result = sampleResult(index)
  if (!result) return { name: 'i-lucide-minus', class: 'text-muted', label: t('routing.rule.regex_editor.no_match') }
  return result.matched
    ? { name: 'i-lucide-check', class: 'text-success', label: t('routing.rule.regex_editor.match') }
    : { name: 'i-lucide-x', class: 'text-muted', label: t('routing.rule.regex_editor.no_match') }
}

function submit(): void {
  if (!canApply.value) return
  emit('close', { pattern: current.value || null })
}
</script>

<template>
  <UModal :title="t('routing.rule.regex_editor.title')" :description="t('routing.rule.regex_editor.description')" :close="{ onClick: () => emit('close', null) }" :ui="{ footer: 'justify-end', content: 'sm:max-w-2xl' }">
    <template #body>
      <div class="space-y-4">
        <UTabs v-model="activeTab" :items="tabItems" size="sm">
          <template #visual>
            <div v-if="builderUsable" class="space-y-3 pt-3">
              <div v-for="(condition, index) in builder.conditions" :key="index" class="flex items-center gap-2">
                <USelect v-model="condition.kind" :items="kindItems(index)" value-key="value" class="w-44 shrink-0" />
                <UInput v-model="condition.value" class="min-w-0 flex-1 font-mono" :placeholder="t('routing.rule.regex_editor.value_placeholder')" />
                <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-x" :aria-label="t('routing.rule.regex_editor.remove_condition')" @click="removeCondition(index)" />
              </div>
              <div class="flex flex-wrap items-center justify-between gap-2">
                <UButton size="xs" color="neutral" variant="outline" icon="i-lucide-plus" :label="t('routing.rule.regex_editor.add_condition')" @click="addCondition" />
                <label class="flex items-center gap-2 text-xs text-muted"><USwitch v-model="builder.caseInsensitive" /> {{ t('routing.rule.regex_editor.case_insensitive') }}</label>
              </div>
              <div class="border border-muted p-3">
                <p class="eyebrow">{{ t('routing.rule.regex_editor.generated_pattern') }}</p>
                <p class="mt-1 break-all font-mono text-xs" :class="pattern ? 'text-highlighted' : 'text-muted'">{{ pattern || '—' }}</p>
              </div>
            </div>
            <div v-else class="space-y-3 pt-3">
              <UAlert color="warning" variant="subtle" :description="t('routing.rule.regex_editor.unparseable_hint')" />
              <UButton size="xs" color="neutral" variant="outline" icon="i-lucide-eraser" :label="t('routing.rule.regex_editor.rebuild')" @click="rebuild" />
            </div>
          </template>
          <template #expert>
            <div class="pt-3">
              <UFormField :label="t('routing.rule.regex_editor.pattern_label')" :description="t('routing.rule.regex_editor.clear_hint')">
                <UInput v-model="pattern" class="w-full font-mono" :placeholder="t('routing.rule.regex_placeholder')" />
              </UFormField>
            </div>
          </template>
        </UTabs>
        <UAlert v-if="invalid" color="error" variant="subtle" :title="t('routing.rule.regex_editor.invalid_pattern')" :description="evaluation?.response.error ?? undefined" :ui="{ description: 'font-mono text-xs break-all' }" />
        <div>
          <p class="eyebrow">{{ t('routing.rule.regex_editor.tester_title') }}</p>
          <div class="mt-2 space-y-2">
            <div v-for="(sample, index) in samples" :key="index" class="flex items-start gap-2">
              <UIcon :name="sampleIcon(index).name" :class="sampleIcon(index).class" :aria-label="sampleIcon(index).label" class="mt-2 size-4 shrink-0" />
              <div class="min-w-0 flex-1">
                <UInput v-model="samples[index]" class="w-full font-mono" :placeholder="t('routing.rule.regex_editor.sample_placeholder')" />
                <p v-if="sampleParts(index)" class="mt-1 truncate font-mono text-[11px] text-muted">{{ sampleParts(index)!.before }}<span class="bg-primary/20 text-primary">{{ sampleParts(index)!.match }}</span>{{ sampleParts(index)!.after }}</p>
              </div>
              <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-x" :aria-label="t('routing.rule.regex_editor.remove_sample')" @click="removeSample(index)" />
            </div>
            <UButton size="xs" color="neutral" variant="outline" icon="i-lucide-plus" :label="t('routing.rule.regex_editor.add_sample')" @click="addSample" />
          </div>
        </div>
      </div>
    </template>
    <template #footer>
      <UButton :label="t('common.actions.cancel')" color="neutral" variant="outline" @click="emit('close', null)" />
      <UButton :label="t('common.actions.apply')" icon="i-lucide-check" :disabled="!canApply" @click="submit" />
    </template>
  </UModal>
</template>
