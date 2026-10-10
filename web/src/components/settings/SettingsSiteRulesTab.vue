<script setup lang="ts">
/**
 * Which release pages this installation recognises, and by which rule (RD-110-08).
 *
 * The list is grouped, because the group is what a person switches when a whole class of sites
 * is not wanted; the switch beside a group heading is the caller's own flex row, as
 * `SectionHeader` requires.
 *
 * Since RD-130-07 every rule here is the person's own: the app brings only a few examples for
 * free sites, switched off (RD-1230-03), and everything else comes from an export somebody made
 * or is written here. So every row can be edited, duplicated and removed, and a copy is how
 * somebody learns from a working rule without touching it — it opens in the editor the moment
 * it exists.
 *
 * Rules travel without a signature (RD-1230-03): the export writes the rules ticked in the list,
 * or all of them, each with its switch; the import shows what a file would do before anything
 * is stored (`SiteRuleImportDialog`), and a rule replaces a stored one only when its box there
 * is ticked. Deleting every rule asks first, naming the count and advising an export.
 *
 * Every row names where its rule came from (RD-1200-05) — an import, the editor, MCP, the
 * example list — as a glyph. A rule the self-test never checked carries no state badge: only
 * what it found is worth a word, and a problem more than "works".
 */
import { useToast } from '@nuxt/ui/composables'
import { computed, nextTick, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SiteRule, SiteRuleDocument, SiteRuleImportPreview, SiteRuleTestResult } from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import SiteRuleEditor from '@/components/settings/SiteRuleEditor.vue'
import SiteRuleImportDialog from '@/components/settings/SiteRuleImportDialog.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useCopyName } from '@/composables/useCopyName'
import { useErrorToast } from '@/composables/useErrorToast'
import { JsonRefusal, useJsonImport } from '@/composables/useJsonImport'
import {
  emptyDraft,
  fromRule,
  useSiteRules,
  type RuleDraft
} from '@/composables/useSiteRules'
import { downloadJson } from '@/utils/jsonFile'
import { editingRowClass } from '@/utils/editingRow'
import { originView } from '@/utils/siteRuleOrigin'

const { t, te } = useI18n()
const toast = useToast()
const showError = useErrorToast()
const confirm = useConfirm()
const rules = useSiteRules()
const copyName = useCopyName()

const draft = ref<RuleDraft>(emptyDraft())
const editingId = ref<string | null>(null)
const testResult = ref<SiteRuleTestResult | null>(null)
const editorElement = ref<HTMLElement | null>(null)
const ruleCount = computed(() => rules.rules.value.length)
/** The rules ticked for the export; none ticked exports all of them. */
const selected = ref<string[]>([])
/** The file being imported and what the service says it would do (RD-1230-03). */
const importDocument = ref<SiteRuleDocument | null>(null)
const importPreview = ref<SiteRuleImportPreview | null>(null)
const importOpen = ref(false)
/** Where the rule open in the editor came from; a new rule has none yet. */
const editingOrigin = computed(() =>
  rules.rules.value.find(entry => entry.id === editingId.value)?.origin ?? null)

onMounted(() => void rules.refresh())

/** The group's own name when a catalogue has one, and the raw word otherwise: groups come
 *  from rule bodies and a rule somebody wrote may carry any word at all. */
function groupLabel(group: string): string {
  const key = `siterules.groups.${group}`
  return te(key) ? t(key) : group
}

/**
 * What the last self-test found here, or nothing: a rule no self-test has checked carries no
 * badge (RD-1230-03). The `checked` day a rule body carries is what its author wrote, so it is
 * not read as a measurement of this installation's reach.
 */
function stateColor(rule: SiteRule): 'success' | 'warning' | 'error' {
  switch (rule.check?.verdict) {
    case 'ok': return 'success'
    case 'structural': return 'warning'
    default: return 'error'
  }
}

/** The sentence behind the badge: the four-way verdict, plus the refusal's own reason. */
function stateTitle(rule: SiteRule): string {
  if (!rule.check) return ''
  const verdict = t(`server.codes.site_rules.state.${rule.check.verdict}`)
  return rule.check.code ? `${verdict} — ${t(`server.codes.${rule.check.code}`)}` : verdict
}

/** Where the rule came from, as the glyph, its word and the sentence behind it (RD-1200-05). */
function originOf(rule: SiteRule): { icon: string, color: 'success' | 'neutral', label: string, detail: string } {
  const view = originView(rule.origin)
  return { icon: view.icon, color: view.color, label: t(view.label), detail: t(view.detail) }
}

function toggleSelected(id: string, checked: boolean | 'indeterminate'): void {
  selected.value = checked === true
    ? [...new Set([...selected.value, id])]
    : selected.value.filter(entry => entry !== id)
}

function startNew(): void {
  draft.value = emptyDraft()
  editingId.value = null
  testResult.value = null
}

function startEdit(rule: SiteRule): void {
  draft.value = fromRule(rule)
  editingId.value = rule.id
  testResult.value = null
  void focusEditor()
}

/**
 * The focus move of `useFormFocus`, skipping the identifier: it is locked while a rule is
 * edited, and a disabled field takes no focus, so the move would otherwise go nowhere.
 */
async function focusEditor(): Promise<void> {
  await nextTick()
  editorElement.value
    ?.querySelector<HTMLElement>('input:not([type="hidden"]):not(:disabled), textarea, select, [role="combobox"]')
    ?.focus()
}

async function save(): Promise<void> {
  if (!await rules.save(draft.value, editingId.value)) return
  toast.add({ title: t('common.actions.save'), color: 'success', icon: 'i-lucide-circle-check' })
  startNew()
}

async function runTest(address: string): Promise<void> {
  testResult.value = await rules.test(draft.value, address)
}

async function duplicate(rule: SiteRule): Promise<void> {
  const id = await rules.duplicate(rule, copyName)
  if (!id) return
  const copy = rules.rules.value.find(entry => entry.id === id)
  if (copy) startEdit(copy)
  toast.add({
    title: t('siterules.duplicated', { name: copy?.name ?? id }),
    color: 'success',
    icon: 'i-lucide-copy-plus'
  })
}

async function remove(rule: SiteRule): Promise<void> {
  const accepted = await confirm({
    title: t('siterules.delete.title'),
    description: t('siterules.delete.description', { name: rule.name }),
    confirmLabel: t('common.actions.delete'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
  if (!accepted) return
  if (await rules.remove(rule.id) && editingId.value === rule.id) startNew()
}

async function exportRules(): Promise<void> {
  // Only ticks that still name a rule: one deleted since must not narrow the export to nothing.
  const ids = selected.value.filter(id => rules.rules.value.some(rule => rule.id === id))
  const document_ = await rules.exportRules(ids)
  if (!document_) return
  downloadJson(document_, 'site-rules')
}

/** The service reads and checks every rule itself; here only that the file is an object. */
const { select: selectFile } = useJsonImport<SiteRuleDocument>({
  check: parsed => typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed)
    ? parsed as SiteRuleDocument
    : new JsonRefusal(t('siterules.transfer.import_unreadable')),
  unreadable: () => t('siterules.transfer.import_unreadable'),
  refuse: message => showError(message),
  take: previewImport
})

async function previewImport(document_: SiteRuleDocument): Promise<void> {
  const preview = await rules.previewImport(document_)
  if (!preview) return
  importDocument.value = document_
  importPreview.value = preview
  importOpen.value = true
}

async function importRules(replace: string[]): Promise<void> {
  if (!importDocument.value) return
  const result = await rules.importRules(importDocument.value, replace)
  if (!result) return
  importOpen.value = false
  const refused = result.rules
    .filter(entry => entry.status === 'refused')
    .map(entry => t('siterules.transfer.refused_one', { name: entry.name || entry.id }))
    .join(', ')
  toast.add({
    title: t('siterules.transfer.imported', { stored: result.stored, replaced: result.replaced, total: result.rules.length }),
    ...(refused ? { description: refused } : {}),
    color: result.stored + result.replaced ? 'success' : 'warning',
    icon: 'i-lucide-file-input'
  })
}

async function clearAll(): Promise<void> {
  const accepted = await confirm({
    title: t('siterules.clear.title'),
    description: t('siterules.clear.description', { count: ruleCount.value }),
    confirmLabel: t('siterules.clear.confirm'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
  if (!accepted) return
  const removed = await rules.clearAll()
  if (removed === null) return
  selected.value = []
  startNew()
  toast.add({ title: t('siterules.clear.done', { count: removed }), color: 'success', icon: 'i-lucide-trash-2' })
}

async function restoreExamples(): Promise<void> {
  const restored = await rules.restoreExamples()
  if (restored === null) return
  toast.add({
    title: restored ? t('siterules.examples.restored', { count: restored }) : t('siterules.examples.none_missing'),
    color: 'success',
    icon: 'i-lucide-lightbulb'
  })
}

/** Behind the list's dots: the bundled rules back, and every rule gone after a question. */
const listMenu = computed(() => [
  { label: t('siterules.examples.restore'), icon: 'i-lucide-lightbulb', onSelect: (): void => { void restoreExamples() } },
  { label: t('siterules.clear.button'), icon: 'i-lucide-trash-2', color: 'error' as const, disabled: !ruleCount.value, onSelect: (): void => { void clearAll() } }
])

/** What the empty list offers: the examples back, a file, or a rule of one's own. */
const emptyActions = computed(() => [
  {
    label: t('siterules.examples.restore'),
    icon: 'i-lucide-lightbulb',
    color: 'primary' as const,
    onClick: (): void => { void restoreExamples() }
  },
  {
    label: t('siterules.editor.title_new'),
    icon: 'i-lucide-file-plus',
    color: 'neutral' as const,
    variant: 'outline' as const,
    onClick: (): void => { void focusEditor() }
  }
])
</script>

<template>
  <div class="space-y-5">
    <SectionHeader
      :eyebrow="t('siterules.header.eyebrow')"
      :title="t('siterules.header.title')"
      :description="t('siterules.header.description')"
      level="page"
    />

    <UAlert
      v-if="rules.error.value"
      color="error"
      icon="i-lucide-circle-alert"
      :description="rules.error.value"
    />

    <FormListLayout :list-title="t('siterules.list.title')" :count="rules.rules.value.length">
      <template #form>
        <div ref="editorElement" data-settings-anchor="siterules.editor">
          <SiteRuleEditor
            v-model="draft"
            :editing-id="editingId"
            :origin="editingOrigin"
            :pending="rules.pending.value"
            :groups="rules.groups.value.map(entry => entry.group)"
            :test-result="testResult"
            @save="save"
            @cancel="startNew"
            @test="runTest"
          />
        </div>
      </template>

      <!-- Export and import as on the other form-and-list pages (`AreaBackupButtons`); the two
           list-wide actions behind the dots, so the row never pushes the count out (owner, 2026-10-10). -->
      <template #list-actions>
        <UButton
          size="sm"
          color="neutral"
          variant="outline"
          icon="i-lucide-download"
          :label="selected.length ? t('siterules.transfer.export_selected', { count: selected.length }) : t('common.backup.export')"
          :disabled="!ruleCount"
          :title="ruleCount ? t('siterules.transfer.export_hint') : t('siterules.transfer.export_empty')"
          @click="exportRules"
        />
        <UFileUpload v-slot="{ open }" :model-value="null" accept=".json" reset :dropzone="false" @update:model-value="selectFile">
          <UButton
            size="sm"
            color="neutral"
            variant="outline"
            icon="i-lucide-file-up"
            :label="t('common.backup.import')"
            :loading="rules.pending.value && !importOpen"
            @click="open()"
          />
        </UFileUpload>
        <UDropdownMenu :items="listMenu">
          <UButton
            size="sm"
            color="neutral"
            variant="ghost"
            icon="i-lucide-ellipsis-vertical"
            :aria-label="t('siterules.list.more')"
            :title="t('siterules.list.more')"
            :loading="rules.busyId.value === 'examples' || rules.busyId.value === 'clear'"
          />
        </UDropdownMenu>
      </template>

      <template #list>
        <!-- The groups in a card, like the editor beside it and the subscriptions list (RD-1110-17). -->
        <UCard as="section" :ui="{ body: 'space-y-4' }">
          <DataState
            :loading="rules.loading.value"
            :error="rules.loadError.value"
            :empty="!rules.rules.value.length"
            :rows="4"
          >
            <UEmpty icon="i-lucide-scan-search" :description="t('siterules.list.empty')" :actions="emptyActions" />
          </DataState>

          <section v-for="entry in rules.byGroup.value" :key="entry.group.group">
            <div class="mb-2 flex items-center justify-between gap-2">
              <h4 class="text-sm font-semibold text-highlighted">{{ groupLabel(entry.group.group) }}</h4>
              <div class="flex items-center gap-2">
                <UBadge color="neutral" variant="outline">{{ entry.group.rules }}</UBadge>
                <USwitch
                  :model-value="entry.group.enabled"
                  :aria-label="t('siterules.group_switch')"
                  :title="t('siterules.group_switch')"
                  :loading="rules.busyId.value === `group:${entry.group.group}`"
                  @update:model-value="(value: boolean) => rules.setGroupEnabled(entry.group.group, value)"
                />
              </div>
            </div>
            <div class="divide-y divide-muted border border-muted">
              <div
                v-for="rule in entry.rules"
                :key="rule.id"
                class="flex flex-wrap items-center gap-3 p-3"
                :class="editingRowClass(editingId === rule.id, 'outline')"
                data-rule-row
              >
                <UCheckbox
                  :model-value="selected.includes(rule.id)"
                  :aria-label="t('siterules.transfer.select', { name: rule.name })"
                  @update:model-value="(checked: boolean | 'indeterminate') => toggleSelected(rule.id, checked)"
                />
                <div class="min-w-0 flex-1">
                  <p class="text-sm font-medium text-highlighted">{{ rule.name }}</p>
                  <p class="truncate font-mono text-2xs text-muted">{{ rule.hosts.join(', ') || rule.id }}</p>
                  <p v-if="rule.description" class="mt-1 line-clamp-2 text-2xs text-muted" :title="rule.description">{{ rule.description }}</p>
                  <p v-if="!entry.group.enabled" class="mt-1 text-2xs text-muted">{{ t('siterules.list.group_off') }}</p>
                </div>
                <!-- One glyph per origin; the word is its name and the sentence its tooltip (RD-1200-05). -->
                <UTooltip :text="originOf(rule).detail">
                  <UBadge
                    :color="originOf(rule).color"
                    variant="subtle"
                    size="sm"
                    :icon="originOf(rule).icon"
                    class="shrink-0"
                    role="img"
                    :aria-label="originOf(rule).label"
                    data-testid="site-rule-origin"
                  />
                </UTooltip>
                <UBadge v-if="editingId === rule.id" size="sm" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
                <!-- Only what a self-test found; "works" stays quiet beside the problems. -->
                <UBadge
                  v-if="rule.check"
                  :color="stateColor(rule)"
                  :variant="rule.check.verdict === 'ok' ? 'outline' : 'subtle'"
                  size="sm"
                  :title="stateTitle(rule)"
                  data-testid="site-rule-state"
                >
                  {{ t(`siterules.badge.${rule.check.verdict}`) }}
                </UBadge>
                <USwitch
                  :model-value="rule.enabled"
                  :aria-label="t('siterules.rule_switch')"
                  :title="t('siterules.rule_switch')"
                  :loading="rules.busyId.value === rule.id"
                  @update:model-value="(value: boolean) => rules.setRuleEnabled(rule, value)"
                />
                <UButton
                  size="xs"
                  color="neutral"
                  variant="ghost"
                  icon="i-lucide-copy-plus"
                  :label="t('common.actions.duplicate')"
                  :title="t('common.duplicate_hint')"
                  :loading="rules.busyId.value === `copy:${rule.id}`"
                  @click="duplicate(rule)"
                />
                <UButton
                  size="xs"
                  color="neutral"
                  variant="ghost"
                  icon="i-lucide-pencil"
                  :aria-label="t('common.actions.edit')"
                  :title="t('common.actions.edit')"
                  @click="startEdit(rule)"
                />
                <UButton
                  size="xs"
                  color="error"
                  variant="ghost"
                  icon="i-lucide-trash-2"
                  :aria-label="t('common.actions.delete')"
                  :title="t('common.actions.delete')"
                  @click="remove(rule)"
                />
              </div>
            </div>
          </section>
        </UCard>
      </template>
    </FormListLayout>

    <SiteRuleImportDialog
      v-model:open="importOpen"
      :preview="importPreview"
      :pending="rules.pending.value"
      @import="importRules"
    />
  </div>
</template>
