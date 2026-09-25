<script setup lang="ts">
/**
 * The editor for one of the person's own site rules (RD-110-08).
 *
 * Every part of a rule is a named field, and the steps are the typed rows `AutomationView`
 * established for its actions: a kind select, the fields that kind needs beside it, and one
 * remove control. The one thing added here is order — a rule's steps run in sequence and read
 * each other's variables, so a step can be moved up and down. JDownloader's LinkCrawler rule
 * editor is the prior art, and its missing half is the reason this component also carries the
 * trial run: an expert form with no way to try the result is where people give up.
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SiteRuleTestResult } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import {
  ENCODINGS,
  PACKAGE_SOURCES,
  STEP_KINDS,
  draftComplete,
  emptyStep,
  type RuleDraft,
  type StepKind
} from '@/composables/useSiteRules'

const props = withDefaults(defineProps<{
  editingId: string | null
  pending: boolean
  testResult: SiteRuleTestResult | null
  /** The groups this installation already has, so the field can offer them (RD-120-21). */
  groups?: string[]
}>(), { groups: () => [] })
const draft = defineModel<RuleDraft>({ required: true })
const emit = defineEmits<{ save: [], cancel: [], test: [string] }>()

const { t } = useI18n()
const address = ref('')
/** Groups named in this session that the list does not hold yet. */
const created = ref<string[]>([])

const kindItems = computed(() =>
  STEP_KINDS.map(kind => ({ label: t(`siterules.steps.kinds.${kind}`), value: kind }))
)
const encodingItems = computed(() =>
  ENCODINGS.map(encoding => ({ label: t(`siterules.steps.encodings.${encoding}`), value: encoding }))
)
const packageItems = computed(() =>
  PACKAGE_SOURCES.map(source => ({ label: t(`siterules.editor.package_${source}`), value: source }))
)
/**
 * The groups on offer: what the installation has, what was named here, and the draft's own —
 * the last one so a rule loaded for editing shows its group rather than an empty field.
 */
const groupItems = computed(() => {
  const named = [...props.groups, ...created.value, draft.value.group]
    .map(group => group.trim())
    .filter(group => group.length > 0)
  return [...new Set(named)].sort((left, right) => left.localeCompare(right))
})
const complete = computed(() => draftComplete(draft.value))
/** The trial run needs an address of its own; the probe is the obvious one to start from. */
const testAddress = computed(() => address.value.trim() || draft.value.probe.trim())

/**
 * A group the list does not hold yet. The field offers what exists so a typo cannot quietly
 * open a second group of one; naming a new one stays possible, because the first rule of a
 * group has to come from somewhere.
 */
function createGroup(name: string): void {
  const group = name.trim()
  if (!group) return
  if (!groupItems.value.includes(group)) created.value.push(group)
  draft.value.group = group
}

function changeKind(index: number, kind: StepKind): void {
  const step = draft.value.steps[index]
  if (!step) return
  draft.value.steps[index] = { ...emptyStep(kind), into: step.into }
}

function move(index: number, by: number): void {
  const target = index + by
  const steps = draft.value.steps
  if (target < 0 || target >= steps.length) return
  const moved = steps[index]
  const displaced = steps[target]
  if (!moved || !displaced) return
  steps[index] = displaced
  steps[target] = moved
}

function verdictColor(verdict: string): 'success' | 'error' | 'neutral' {
  if (verdict === 'confirmed' || verdict === 'claimed') return 'success'
  if (verdict === 'not-a-file') return 'error'
  return 'neutral'
}
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <SectionHeader
      :eyebrow="t('siterules.editor.eyebrow')"
      :title="props.editingId ? t('siterules.editor.title_edit', { name: draft.name || props.editingId }) : t('siterules.editor.title_new')"
      :description="t('siterules.editor.description')"
      level="sub"
    />

    <div class="mt-4 grid gap-3">
      <UFormField :label="t('siterules.editor.id')" :description="t('siterules.editor.id_hint')">
        <UInput v-model="draft.id" :disabled="!!props.editingId" class="w-full font-mono" placeholder="my-board" />
      </UFormField>
      <UFormField :label="t('siterules.editor.name')">
        <UInput v-model="draft.name" maxlength="120" class="w-full" />
      </UFormField>
      <UFormField :label="t('siterules.editor.group')" :description="t('siterules.editor.group_hint')">
        <UInputMenu
          v-model="draft.group"
          :items="groupItems"
          create-item
          :aria-label="t('siterules.editor.group')"
          class="w-full font-mono"
          placeholder="board"
          @create="createGroup"
        />
      </UFormField>
      <UFormField :label="t('siterules.editor.version')" :description="t('siterules.editor.version_hint')">
        <UInput v-model.number="draft.version" type="number" min="1" class="w-full" />
      </UFormField>
      <UFormField :label="t('siterules.editor.hosts')" :description="t('siterules.editor.hosts_hint')">
        <UTextarea v-model="draft.hosts" :rows="2" class="w-full font-mono text-xs" placeholder="example.org" />
      </UFormField>
      <UFormField :label="t('siterules.editor.paths')" :description="t('siterules.editor.paths_hint')">
        <UTextarea v-model="draft.paths" :rows="2" class="w-full font-mono text-xs" />
      </UFormField>
      <UFormField :label="t('siterules.editor.dead')" :description="t('siterules.editor.dead_hint')">
        <UTextarea v-model="draft.dead" :rows="2" class="w-full font-mono text-xs" />
      </UFormField>
      <UFormField :label="t('siterules.editor.probe')" :description="t('siterules.editor.probe_hint')">
        <UInput v-model="draft.probe" class="w-full font-mono text-xs" placeholder="https://example.org/release/1" />
      </UFormField>
      <UFormField :label="t('siterules.editor.checked')">
        <UInput v-model="draft.checked" type="date" class="w-full" />
      </UFormField>
      <UCheckbox
        v-model="draft.mirrors"
        :label="t('siterules.editor.mirrors')"
        :description="t('siterules.editor.mirrors_hint')"
      />
    </div>

    <h4 class="mt-5 mb-1 text-sm font-medium text-highlighted">{{ t('siterules.editor.package_heading') }}</h4>
    <div class="grid gap-3">
      <UFormField :label="t('siterules.editor.package_source')">
        <USelect v-model="draft.packageFrom" :items="packageItems" class="w-full" />
      </UFormField>
      <UFormField v-if="draft.packageFrom === 'regex'" :label="t('siterules.editor.package_pattern')">
        <UInput v-model="draft.packagePattern" class="w-full font-mono text-xs" />
      </UFormField>
      <UFormField v-if="draft.packageFrom === 'regex'" :label="t('siterules.editor.package_from')">
        <UInput v-model="draft.packageSource" class="w-full font-mono text-xs" placeholder="page" />
      </UFormField>
      <UFormField v-if="draft.packageFrom === 'variable'" :label="t('siterules.editor.package_name')">
        <UInput v-model="draft.packageName" class="w-full font-mono text-xs" />
      </UFormField>
    </div>

    <SectionHeader
      class="mt-5"
      :eyebrow="t('siterules.steps.heading')"
      :title="t('siterules.steps.heading')"
      :description="t('siterules.steps.description')"
      level="sub"
    />
    <div class="mt-2 space-y-2">
      <div
        v-for="(step, index) in draft.steps"
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
              :disabled="index === draft.steps.length - 1"
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
              @click="draft.steps.splice(index, 1)"
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
          <UFormField v-if="step.kind === 'captcha'" :label="t('siterules.steps.challenge')">
            <UInput v-model="step.challenge" class="w-full font-mono text-xs" placeholder="recaptcha-v2" />
          </UFormField>
          <UFormField v-if="step.kind === 'captcha'" :label="t('siterules.steps.sitekey')">
            <UInput v-model="step.sitekey" class="w-full font-mono text-xs" />
          </UFormField>
          <UFormField :label="t('siterules.steps.into')">
            <UInput v-model="step.into" class="w-full font-mono text-xs" placeholder="links" />
          </UFormField>
          <UCheckbox v-if="step.kind === 'regex'" v-model="step.all" :label="t('siterules.steps.all')" />
        </div>
      </div>
      <p v-if="!draft.steps.length" class="text-xs text-error">{{ t('siterules.steps.empty') }}</p>
    </div>
    <UButton
      class="mt-2"
      size="xs"
      color="neutral"
      variant="soft"
      icon="i-lucide-plus"
      :label="t('siterules.steps.add')"
      @click="draft.steps.push(emptyStep())"
    />

    <SectionHeader
      class="mt-5"
      :eyebrow="t('siterules.test.heading')"
      :title="t('siterules.test.heading')"
      :description="t('siterules.test.description')"
      level="sub"
    />
    <div class="mt-2 flex flex-wrap items-end gap-2">
      <UFormField class="min-w-64 flex-1" :label="t('siterules.test.address')">
        <UInput v-model="address" class="w-full font-mono text-xs" :placeholder="draft.probe || 'https://example.org/release/1'" />
      </UFormField>
      <UButton
        color="neutral"
        variant="outline"
        icon="i-lucide-flask-conical"
        :label="t('siterules.test.run')"
        :loading="props.pending"
        :disabled="!complete || !testAddress"
        @click="emit('test', testAddress)"
      />
    </div>
    <div v-if="props.testResult" class="mt-3 border border-muted p-3">
      <p class="truncate font-mono text-xs text-muted">{{ t('siterules.test.crawled', { address: props.testResult.address }) }}</p>
      <p v-if="props.testResult.error" role="alert" class="mt-2 text-sm text-error">
        {{ t(`server.codes.${props.testResult.error}`) }}
      </p>
      <template v-else>
        <div class="mt-2 flex flex-wrap items-center gap-2 text-xs">
          <UBadge color="neutral" variant="outline">{{ t('siterules.test.pages', props.testResult.pages_fetched) }}</UBadge>
          <UBadge color="success" variant="subtle">{{ t('siterules.test.kept', { count: props.testResult.kept }) }}</UBadge>
          <UBadge v-if="props.testResult.refused" color="error" variant="subtle">{{ t('siterules.test.refused', { count: props.testResult.refused }) }}</UBadge>
          <UBadge v-if="props.testResult.mirrors" color="neutral" variant="subtle" :title="t('siterules.test.mirrors')">{{ t('siterules.test.mirrors') }}</UBadge>
        </div>
        <p class="mt-2 text-xs text-muted">
          {{ props.testResult.package_name
            ? `${t('siterules.test.package')}: ${props.testResult.package_name}`
            : t('siterules.test.package_none') }}
        </p>
        <p v-if="!props.testResult.links.length" class="mt-2 text-xs text-muted">{{ t('siterules.test.no_links') }}</p>
        <ul v-else class="mt-2 space-y-1">
          <li v-for="link in props.testResult.links" :key="link.url" class="flex flex-wrap items-center gap-2">
            <span class="min-w-0 flex-1 truncate font-mono text-[11px]">{{ link.url }}</span>
            <UBadge
              :color="verdictColor(link.verdict)"
              variant="subtle"
              :title="link.code ? t(`server.codes.${link.code}`) : t(`siterules.test.verdicts.${link.verdict}`)"
            >
              {{ t(`siterules.test.verdicts.${link.verdict}`) }}
            </UBadge>
          </li>
        </ul>
      </template>
    </div>

    <div class="mt-5 flex flex-wrap items-center gap-2">
      <UCheckbox v-model="draft.enabled" :label="t('siterules.editor.enabled')" />
      <div class="ml-auto flex gap-2">
        <UButton
          v-if="props.editingId"
          color="neutral"
          variant="ghost"
          :label="t('common.actions.cancel')"
          @click="emit('cancel')"
        />
        <UButton
          icon="i-lucide-save"
          :label="t('common.actions.save')"
          :disabled="!complete"
          :loading="props.pending"
          @click="emit('save')"
        />
      </div>
    </div>
    <p v-if="!complete" class="mt-2 text-xs text-muted">{{ t('siterules.editor.incomplete') }}</p>
  </section>
</template>
