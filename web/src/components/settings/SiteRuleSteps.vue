<script setup lang="ts">
/**
 * The steps of a site rule as typed rows (RD-110-08): a kind select, the fields that kind needs
 * beside it, order controls and one remove control. Split out of `SiteRuleEditor` when a rule
 * could carry a second step list — the steps each group runs (RD-1170-02) — so both lists are
 * the same rows.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  ENCODINGS,
  STEP_KINDS,
  emptyStep,
  type StepDraft,
  type StepKind
} from '@/composables/useSiteRules'

const steps = defineModel<StepDraft[]>({ required: true })
/** The kind a newly added step starts as. */
const props = withDefaults(defineProps<{ addKind?: StepKind }>(), { addKind: 'fetch' })

const { t } = useI18n()

const kindItems = computed(() =>
  STEP_KINDS.map(kind => ({ label: t(`siterules.steps.kinds.${kind}`), value: kind }))
)
const encodingItems = computed(() =>
  ENCODINGS.map(encoding => ({ label: t(`siterules.steps.encodings.${encoding}`), value: encoding }))
)

function changeKind(index: number, kind: StepKind): void {
  const step = steps.value[index]
  if (!step) return
  steps.value[index] = { ...emptyStep(kind), into: step.into }
}

function move(index: number, by: number): void {
  const target = index + by
  const list = steps.value
  if (target < 0 || target >= list.length) return
  const moved = list[index]
  const displaced = list[target]
  if (!moved || !displaced) return
  list[index] = displaced
  list[target] = moved
}
</script>

<template>
  <div>
    <div class="mt-2 space-y-2">
      <div
        v-for="(step, index) in steps"
        :key="index"
        class="border border-muted p-3"
      >
        <div class="flex flex-wrap items-center gap-2">
          <span class="font-mono text-xs text-muted">{{ index + 1 }}</span>
          <USelect
            :model-value="step.kind"
            :items="kindItems"
            :aria-label="t('siterules.steps.kind')"
            class="w-56"
            @update:model-value="(value: StepKind) => changeKind(index, value)"
          />
          <div class="ml-auto flex items-center gap-1">
            <UButton
              size="xs"
              color="neutral"
              variant="ghost"
              icon="i-lucide-chevron-up"
              :disabled="index === 0"
              :aria-label="t('siterules.steps.up')"
              :title="t('siterules.steps.up')"
              @click="move(index, -1)"
            />
            <UButton
              size="xs"
              color="neutral"
              variant="ghost"
              icon="i-lucide-chevron-down"
              :disabled="index === steps.length - 1"
              :aria-label="t('siterules.steps.down')"
              :title="t('siterules.steps.down')"
              @click="move(index, 1)"
            />
            <UButton
              size="xs"
              color="error"
              variant="ghost"
              icon="i-lucide-x"
              :aria-label="t('siterules.steps.remove')"
              :title="t('siterules.steps.remove')"
              @click="steps.splice(index, 1)"
            />
          </div>
        </div>
        <div class="mt-2 grid gap-3">
          <UFormField
            v-if="['fetch', 'fetch-json', 'form'].includes(step.kind)"
            :label="t('siterules.steps.url')"
            :description="step.kind === 'fetch' ? t('siterules.steps.url_hint') : undefined"
          >
            <UInput v-model="step.url" class="w-full font-mono text-xs" />
          </UFormField>
          <UFormField v-if="step.kind === 'fetch-json'" :label="t('siterules.steps.path')">
            <UInput v-model="step.path" class="w-full font-mono text-xs" placeholder="/items/0/url" />
          </UFormField>
          <UFormField v-if="step.kind === 'regex'" :label="t('siterules.steps.pattern')">
            <UInput v-model="step.pattern" class="w-full font-mono text-xs" />
          </UFormField>
          <UFormField v-if="['regex', 'decode', 'redirect'].includes(step.kind)" :label="t('siterules.steps.from')">
            <UInput v-model="step.from" class="w-full font-mono text-xs" placeholder="page" />
          </UFormField>
          <UFormField v-if="step.kind === 'decode'" :label="t('siterules.steps.encoding')">
            <USelect v-model="step.encoding" :items="encodingItems" class="w-full" />
          </UFormField>
          <UFormField v-if="step.kind === 'form'" :label="t('siterules.steps.fields')" :description="t('siterules.steps.fields_hint')">
            <UTextarea v-model="step.fields" :rows="2" class="w-full font-mono text-xs" />
          </UFormField>
          <UCheckbox v-if="step.kind === 'form'" v-model="step.json" :label="t('siterules.steps.json')" :description="t('siterules.steps.json_hint')" />
          <UFormField v-if="step.kind === 'captcha'" :label="t('siterules.steps.challenge')">
            <UInput v-model="step.challenge" class="w-full font-mono text-xs" placeholder="recaptcha-v2" />
          </UFormField>
          <UFormField v-if="step.kind === 'captcha'" :label="t('siterules.steps.sitekey')">
            <UInput v-model="step.sitekey" class="w-full font-mono text-xs" />
          </UFormField>
          <UFormField v-if="step.kind === 'captcha'" :label="t('siterules.steps.page')" :description="t('siterules.steps.page_hint')">
            <UInput v-model="step.page" class="w-full font-mono text-xs" placeholder="${url}" />
          </UFormField>
          <UCheckbox v-if="step.kind === 'captcha'" v-model="step.invisible" :label="t('siterules.steps.invisible')" />
          <UFormField :label="t('siterules.steps.into')">
            <UInput v-model="step.into" class="w-full font-mono text-xs" placeholder="links" />
          </UFormField>
          <UCheckbox v-if="step.kind === 'regex'" v-model="step.all" :label="t('siterules.steps.all')" />
        </div>
      </div>
      <p v-if="!steps.length" class="text-xs text-error">{{ t('siterules.steps.empty') }}</p>
    </div>
    <UButton
      class="mt-2"
      size="xs"
      color="neutral"
      variant="soft"
      icon="i-lucide-plus"
      :label="t('siterules.steps.add')"
      @click="steps.push(emptyStep(props.addKind))"
    />
  </div>
</template>
