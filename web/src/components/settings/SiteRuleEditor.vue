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
 *
 * A page that lists several releases is one switch away (RD-1170-02): the rule's steps leave
 * one entry per release in a variable, and a second step list turns each entry into a package
 * of its own, with its own name and its hosters as mirrors.
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SiteRuleTestResult as TestResult } from '@/api/types'
import DateField from '@/components/DateField.vue'
import FormActions from '@/components/FormActions.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import SiteRulePackageFields from '@/components/settings/SiteRulePackageFields.vue'
import SiteRuleSteps from '@/components/settings/SiteRuleSteps.vue'
import SiteRuleTestResult from '@/components/settings/SiteRuleTestResult.vue'
import { GROUP_MIRRORS, draftComplete, type RuleDraft } from '@/composables/useSiteRules'
import { PLAIN } from '@/utils/numberInput'

const props = withDefaults(defineProps<{
  editingId: string | null
  pending: boolean
  testResult: TestResult | null
  /** The groups this installation already has, so the field can offer them (RD-120-21). */
  groups?: string[]
}>(), { groups: () => [] })
const draft = defineModel<RuleDraft>({ required: true })
const emit = defineEmits<{ save: [], cancel: [], test: [string] }>()

const { t } = useI18n()
const address = ref('')
/** Groups named in this session that the list does not hold yet. */
const created = ref<string[]>([])

/** How one entry's links pair up as copies; written out so every label is a literal key. */
const mirrorItems = computed(() => {
  const labels = {
    none: t('siterules.packages.mirrors_none'),
    'by-host': t('siterules.packages.mirrors_by_host'),
    all: t('siterules.packages.mirrors_all')
  }
  return GROUP_MIRRORS.map(mode => ({ label: labels[mode], value: mode }))
})
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

function submit(): void {
  if (complete.value) emit('save')
}

function runTest(): void {
  if (complete.value && testAddress.value) emit('test', testAddress.value)
}
</script>

<template>
  <UCard as="section">
    <SectionHeader
      :eyebrow="t('siterules.editor.eyebrow')"
      :title="props.editingId ? t('siterules.editor.title_edit', { name: draft.name || props.editingId }) : t('siterules.editor.title_new')"
      :description="t('siterules.editor.description')"
      level="sub"
    />

    <!-- One form for the whole rule, so Enter in any field saves it; Enter in the test address
         runs the test instead, because that is what the reader typed the address for. -->
    <form class="mt-4" @submit.prevent="submit">
      <div class="grid gap-3">
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
          <UInputNumber v-model="draft.version" required :min="1" :format-options="PLAIN" class="w-full" />
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
          <DateField v-model="draft.checked" class="w-full" />
        </UFormField>
        <!-- With groups, mirrors are stated per group; the service refuses both at once. -->
        <UCheckbox
          v-model="draft.mirrors"
          :disabled="draft.grouped"
          :label="t('siterules.editor.mirrors')"
          :description="t('siterules.editor.mirrors_hint')"
        />
      </div>

      <h4 class="mt-5 mb-1 text-sm font-medium text-highlighted">{{ t('siterules.editor.package_heading') }}</h4>
      <SiteRulePackageFields v-model="draft" />

      <SectionHeader
        class="mt-5"
        :eyebrow="t('siterules.steps.heading')"
        :title="t('siterules.steps.heading')"
        :description="t('siterules.steps.description')"
        level="sub"
      />
      <SiteRuleSteps v-model="draft.steps" />

      <SectionHeader
        class="mt-5"
        :eyebrow="t('siterules.packages.heading')"
        :title="t('siterules.packages.heading')"
        :description="t('siterules.packages.description')"
        level="sub"
      />
      <USwitch v-model="draft.grouped" class="mt-2" :label="t('siterules.packages.enabled')" />
      <div v-if="draft.grouped" class="mt-3 grid gap-3">
        <UFormField :label="t('siterules.packages.from')" :description="t('siterules.packages.from_hint')">
          <UInput v-model="draft.groups.from" class="w-full font-mono text-xs" placeholder="releases" />
        </UFormField>
        <UFormField :label="t('siterules.packages.into')" :description="t('siterules.packages.into_hint')">
          <UInput v-model="draft.groups.into" class="w-full font-mono text-xs" placeholder="entry" />
        </UFormField>
        <UFormField :label="t('siterules.packages.pick')" :description="t('siterules.packages.pick_hint')">
          <UTextarea v-model="draft.groups.pick" :rows="3" class="w-full font-mono text-xs" placeholder="season=&quot;season&quot;:(\d+)" />
        </UFormField>
        <h4 class="text-sm font-medium text-highlighted">{{ t('siterules.packages.steps') }}</h4>
        <SiteRuleSteps v-model="draft.groups.steps" add-kind="regex" />
        <h4 class="text-sm font-medium text-highlighted">{{ t('siterules.packages.package') }}</h4>
        <SiteRulePackageFields v-model="draft.groups" default-source="page" />
        <UFormField :label="t('siterules.packages.mirrors')" :description="t('siterules.packages.mirrors_hint')">
          <USelect v-model="draft.groups.mirrors" :items="mirrorItems" class="w-full" />
        </UFormField>
      </div>

      <SectionHeader
        class="mt-5"
        :eyebrow="t('siterules.test.heading')"
        :title="t('siterules.test.heading')"
        :description="t('siterules.test.description')"
        level="sub"
      />
      <div class="mt-2 flex flex-wrap items-end gap-2">
        <UFormField class="min-w-64 flex-1" :label="t('siterules.test.address')">
          <UInput
            v-model="address"
            class="w-full font-mono text-xs"
            :placeholder="draft.probe || 'https://example.org/release/1'"
            @keydown.enter.prevent="runTest"
          />
        </UFormField>
        <UButton
          color="neutral"
          variant="outline"
          icon="i-lucide-flask-conical"
          :label="t('siterules.test.run')"
          :loading="props.pending"
          :disabled="!complete || !testAddress"
          @click="runTest"
        />
      </div>
      <SiteRuleTestResult v-if="props.testResult" :result="props.testResult" />

      <USwitch v-model="draft.enabled" class="mt-5" :label="t('siterules.editor.enabled')" />
      <FormActions
        class="mt-3"
        :editing="!!props.editingId"
        :create-label="t('siterules.editor.create')"
        create-icon="i-lucide-file-plus"
        :disabled="!complete"
        :loading="props.pending"
        @cancel="emit('cancel')"
      />
      <p v-if="!complete" class="mt-2 text-xs text-muted">{{ t('siterules.editor.incomplete') }}</p>
    </form>
  </UCard>
</template>
