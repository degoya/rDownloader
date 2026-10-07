<script setup lang="ts">
import { watchDebounced } from '@vueuse/core'
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { TestRegexResponse, TestRegexSampleResult } from '@/api/types'
import RegexDiagram from '@/components/routing/RegexDiagram.vue'
import type { RegexTarget } from '@/composables/useRegexEditor'
import type { RegexConditionKind } from '@/utils/regexBuilder'
import { allowedKinds, buildPattern, parsePattern } from '@/utils/regexBuilder'

const props = defineProps<{
  pattern: string | null
  /** The name a category rule matches (RD-1140-02): the tester's samples and title follow it. */
  target?: RegexTarget | undefined
}>()
const emit = defineEmits<{ close: [result: { pattern: string | null } | null] }>()
const { t } = useI18n()

const EMPTY_CONDITION = { kind: 'contains' as RegexConditionKind, value: '' }
const initial = props.pattern ? parsePattern(props.pattern) : { conditions: [{ ...EMPTY_CONDITION }], caseInsensitive: false }
const pattern = ref(props.pattern ?? '')
const builder = reactive(initial ?? { conditions: [], caseInsensitive: false })
const builderUsable = ref(initial !== null)
const activeTab = ref(initial ? 'visual' : 'expert')
const SAMPLES: Record<RegexTarget, string[]> = {
  file: ['Movie.2024.1080p.x265.mkv', 'Show.S01E01.720p.mp4'],
  package: ['Movie.2024.1080p.BluRay-GROUP', 'Show.S01.720p.WEB-GROUP'],
  either: ['Movie.2024.1080p.x265.mkv', 'Movie.2024.1080p.BluRay-GROUP']
}
const target = props.target ?? 'file'
const samples = ref([...SAMPLES[target]])
const testerTitle = computed(() => t(target === 'file' ? 'routing.rule.regex_editor.tester_title' : `routing.rule.regex_editor.tester_title_${target}`))
const samplePlaceholder = computed(() => t(target === 'package' ? 'routing.rule.regex_editor.package_sample_placeholder' : 'routing.rule.regex_editor.sample_placeholder'))

/** Last backend evaluation with the inputs it was computed for, so stale results never render. */
interface Evaluation { pattern: string, samples: string[], response: TestRegexResponse }
const evaluation = ref<Evaluation | null>(null)
let sequence = 0

const current = computed(() => pattern.value.trim())
const upToDate = computed(() => !current.value || evaluation.value?.pattern === current.value)
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
  const requestedReplacement = replacing ? replaceWith.value : null
  const response = await api.POST('/api/v1/category-rules/test-regex', {
    body: { pattern: requested, samples: requestedSamples, ...(replacing ? { replacement: requestedReplacement } : {}) }
  })
  if (id !== sequence || !response.data) return
  evaluation.value = { pattern: requested, samples: requestedSamples, response: response.data }
  replacedWith.value = requestedReplacement
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

/**
 * The replacement mode (RD-1140-05): given a `replacement`, the editor edits a package-name regex
 * rule — a pattern and what every match becomes — and shows what each sample turns into, from
 * the same engine the rule runs with. Without it, nothing here applies.
 */
const replacement = defineModel<string | null | undefined>('replacement')
const replacing = typeof replacement.value === 'string'
const replaceWith = ref(replacement.value ?? '')
/** The replacement the shown results were computed with. */
const replacedWith = ref<string | null>(null)
const PACKAGE_NAME_SAMPLES = ['Big Buck Bunny [1080p]', 'Sintel_Directors_Cut_Update_v1.0.2_EXAMPLE']
if (replacing) samples.value = [...PACKAGE_NAME_SAMPLES]
const modalTitle = computed(() => t(replacing ? 'routing.rule.regex_editor.replacement_title' : 'routing.rule.regex_editor.title'))
watchDebounced(replaceWith, () => void evaluate(), { debounce: 300 })

/** What sample `index` becomes, once the shown evaluation is the current one. */
function replacedText(index: number): string | null {
  if (replacedWith.value !== replaceWith.value) return null
  return sampleResult(index)?.replaced ?? null
}

function submit(): void {
  if (!canApply.value) return
  const result = replacing ? { pattern: current.value || null, replacement: replaceWith.value } : { pattern: current.value || null }
  emit('close', result)
}
</script>

<template>
  <UModal :title="modalTitle" :description="t('routing.rule.regex_editor.description')" :close="{ onClick: () => emit('close', null) }" :ui="{ content: 'sm:max-w-2xl' }">
    <template #body>
      <div class="space-y-4">
        <UTabs v-model="activeTab" :items="tabItems" size="sm">
          <template #visual>
            <div v-if="builderUsable" class="space-y-3">
              <div v-for="(condition, index) in builder.conditions" :key="index" class="flex items-center gap-2">
                <USelect v-model="condition.kind" :items="kindItems(index)" value-key="value" class="w-44 shrink-0" />
                <UInput v-model="condition.value" class="min-w-0 flex-1 font-mono" :placeholder="t('routing.rule.regex_editor.value_placeholder')" />
                <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-x" :aria-label="t('routing.rule.regex_editor.remove_condition')" @click="removeCondition(index)" />
              </div>
              <div class="flex flex-wrap items-center justify-between gap-2">
                <UButton size="xs" color="neutral" variant="outline" icon="i-lucide-plus" :label="t('routing.rule.regex_editor.add_condition')" @click="addCondition" />
                <USwitch v-model="builder.caseInsensitive" size="sm" :label="t('routing.rule.regex_editor.case_insensitive')" />
              </div>
              <div class="border border-muted p-3">
                <p class="eyebrow">{{ t('routing.rule.regex_editor.generated_pattern') }}</p>
                <p class="mt-1 break-all font-mono text-xs" :class="pattern ? 'text-highlighted' : 'text-muted'">{{ pattern || '—' }}</p>
              </div>
            </div>
            <div v-else class="space-y-3">
              <UAlert color="warning" :description="t('routing.rule.regex_editor.unparseable_hint')" />
              <UButton size="xs" color="neutral" variant="outline" icon="i-lucide-eraser" :label="t('routing.rule.regex_editor.rebuild')" @click="rebuild" />
            </div>
          </template>
          <template #expert>
            <div>
              <UFormField :label="t('routing.rule.regex_editor.pattern_label')" :description="t('routing.rule.regex_editor.clear_hint')">
                <UInput v-model="pattern" class="w-full font-mono" :placeholder="t('routing.rule.regex_placeholder')" />
              </UFormField>
            </div>
          </template>
        </UTabs>
        <UFormField v-if="replacing" :label="t('routing.rule.regex_editor.replacement_label')" :description="t('routing.rule.regex_editor.replacement_description')">
          <UInput v-model="replaceWith" class="w-full font-mono" placeholder="$1" data-testid="regex-replacement" />
        </UFormField>
        <RegexDiagram v-if="current" :response="evaluation?.response ?? null" :stale="!upToDate" />
        <div>
          <p class="eyebrow">{{ testerTitle }}</p>
          <div class="mt-2 space-y-2">
            <div v-for="(sample, index) in samples" :key="index" class="flex items-start gap-2">
              <UIcon :name="sampleIcon(index).name" :class="sampleIcon(index).class" :aria-label="sampleIcon(index).label" class="mt-2 size-4 shrink-0" />
              <div class="min-w-0 flex-1">
                <UInput v-model="samples[index]" class="w-full font-mono" :placeholder="samplePlaceholder" />
                <p v-if="sampleParts(index)" class="mt-1 truncate font-mono text-2xs text-muted">{{ sampleParts(index)!.before }}<span class="bg-primary/20 text-primary">{{ sampleParts(index)!.match }}</span>{{ sampleParts(index)!.after }}</p>
              </div>
              <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-x" :aria-label="t('routing.rule.regex_editor.remove_sample')" @click="removeSample(index)" />
            </div>
            <UButton size="xs" color="neutral" variant="outline" icon="i-lucide-plus" :label="t('routing.rule.regex_editor.add_sample')" @click="addSample" />
          </div>
        </div>
        <ul v-if="replacing" class="space-y-1" data-testid="regex-replaced">
          <template v-for="(sample, index) in samples" :key="index">
            <li v-if="replacedText(index) !== null" class="break-all font-mono text-2xs text-muted">
              {{ sample }} <span class="font-sans">{{ t('routing.rule.regex_editor.replaced_as') }}</span> <span class="text-highlighted">{{ replacedText(index) }}</span>
            </li>
          </template>
        </ul>
      </div>
    </template>
    <template #footer>
      <UButton :label="t('common.actions.cancel')" color="neutral" variant="outline" @click="emit('close', null)" />
      <UButton :label="t('common.actions.apply')" icon="i-lucide-check" :disabled="!canApply" @click="submit" />
    </template>
  </UModal>
</template>
