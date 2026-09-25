<script setup lang="ts">
/**
 * Which release pages this installation recognises, and by which rule (RD-110-08).
 *
 * The list is grouped, because the group is what a person switches when a whole class of sites
 * is not wanted; the switch beside a group heading is the caller's own flex row, as
 * `SectionHeader` requires.
 *
 * Since RD-130-07 every rule here is the person's own: nothing ships with the binary, and the
 * project's rules arrive through the import of the signed file every release carries. So every
 * row can be edited, duplicated and removed, and a copy is how somebody learns from a working
 * rule without touching it — it opens in the editor the moment it exists.
 *
 * Import is deliberately unfriendly in one respect: a rule from a file arrives switched off,
 * signed or not, the confirmation is the switch, and the server enforces that rather than this
 * component. The file goes out exactly as it was read, because a signature covers bytes.
 */
import { useToast } from '@nuxt/ui/composables'
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SiteRule, SiteRuleTestResult } from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import SiteRuleEditor from '@/components/settings/SiteRuleEditor.vue'
import { useConfirm } from '@/composables/useConfirm'
import {
  emptyDraft,
  fromRule,
  useSiteRules,
  type RuleDraft
} from '@/composables/useSiteRules'

const { t, te } = useI18n()
const toast = useToast()
const confirm = useConfirm()
const rules = useSiteRules()

const draft = ref<RuleDraft>(emptyDraft())
const editingId = ref<string | null>(null)
const testResult = ref<SiteRuleTestResult | null>(null)
const fileInput = ref<HTMLInputElement | null>(null)
const ruleCount = computed(() => rules.rules.value.length)

onMounted(() => void rules.refresh())

/** The group's own name when a catalogue has one, and the raw word otherwise: groups come
 *  from rule bodies and a rule somebody wrote may carry any word at all. */
function groupLabel(group: string): string {
  const key = `siterules.groups.${group}`
  return te(key) ? t(key) : group
}

/**
 * Which of the five states this rule is in: what the last self-test found here, or that none
 * has run. The `checked` day a rule body carries is what its author wrote, so it is not read
 * as a measurement of this installation's reach (RD-130-07 retired the `verified` state that
 * trusted it for the rules of the compiled-in pack).
 */
function stateKey(rule: SiteRule): string {
  return rule.check ? rule.check.verdict : 'unknown'
}

function stateColor(rule: SiteRule): 'success' | 'warning' | 'error' | 'neutral' {
  switch (stateKey(rule)) {
    case 'ok': return 'success'
    case 'structural': return 'warning'
    case 'blocked': return 'error'
    case 'dead': return 'error'
    default: return 'neutral'
  }
}

/** The sentence behind the badge: the four-way verdict, plus the refusal's own reason. */
function stateTitle(rule: SiteRule): string {
  if (!rule.check) return t('siterules.badge.unknown')
  const verdict = t(`server.codes.site_rules.state.${rule.check.verdict}`)
  return rule.check.code ? `${verdict} — ${t(`server.codes.${rule.check.code}`)}` : verdict
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
  const id = await rules.duplicate(rule, t('siterules.copy_suffix'))
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
  const document_ = await rules.exportRules()
  if (!document_) return
  const blob = new Blob([JSON.stringify(document_, null, 2)], { type: 'application/json' })
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = `rdownloader-site-rules-${new Date().toISOString().slice(0, 10)}.json`
  document.body.append(anchor)
  anchor.click()
  anchor.remove()
  URL.revokeObjectURL(url)
}

function chooseFile(): void {
  if (!fileInput.value) return
  fileInput.value.value = ''
  fileInput.value.click()
}

async function selectFile(event: Event): Promise<void> {
  const target = event.target
  const file = target instanceof HTMLInputElement ? target.files?.item(0) : null
  if (!file) return
  const text = await file.text()
  try {
    JSON.parse(text)
  } catch {
    toast.add({ title: t('siterules.transfer.import_unreadable'), color: 'error', icon: 'i-lucide-circle-alert' })
    return
  }
  const result = await rules.importRules(text)
  if (!result) return
  const refused = result.rules
    .filter(entry => entry.code)
    .map(entry => t('siterules.transfer.refused_one', { name: entry.name || entry.id }))
    .join(', ')
  const imported = result.signed ? 'siterules.transfer.imported_signed' : 'siterules.transfer.imported'
  toast.add({
    title: t(imported, { stored: result.stored, total: result.rules.length }),
    ...(refused ? { description: refused } : {}),
    color: result.stored ? 'success' : 'warning',
    icon: 'i-lucide-file-input'
  })
}
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
      variant="subtle"
      icon="i-lucide-circle-alert"
      :description="rules.error.value"
    />

    <FormListLayout :list-title="t('siterules.list.title')" :count="rules.rules.value.length">
      <template #form>
        <SiteRuleEditor
          v-model="draft"
          :editing-id="editingId"
          :pending="rules.pending.value"
          :groups="rules.groups.value.map(entry => entry.group)"
          :test-result="testResult"
          @save="save"
          @cancel="startNew"
          @test="runTest"
        />
      </template>

      <template #list-actions>
        <UButton
          size="xs"
          color="neutral"
          variant="ghost"
          icon="i-lucide-plus"
          :label="t('siterules.new')"
          @click="startNew"
        />
        <UButton
          size="xs"
          color="neutral"
          variant="ghost"
          icon="i-lucide-download"
          :label="t('siterules.transfer.export')"
          :disabled="!ruleCount"
          :title="ruleCount ? t('siterules.transfer.export') : t('siterules.transfer.export_empty')"
          @click="exportRules"
        />
        <UButton
          size="xs"
          color="neutral"
          variant="ghost"
          icon="i-lucide-upload"
          :label="t('siterules.transfer.import')"
          :title="t('siterules.transfer.import_hint')"
          @click="chooseFile"
        />
        <input ref="fileInput" class="hidden" type="file" accept=".json,application/json" @change="selectFile">
      </template>

      <template #list>
        <p class="mb-3 text-xs leading-5 text-muted">{{ t('siterules.transfer.import_hint') }}</p>
        <DataState
          :loading="rules.loading.value"
          :error="rules.loadError.value"
          :empty="!rules.rules.value.length"
          :rows="4"
        >
          <p class="border border-dashed border-muted p-5 text-center text-sm text-muted">
            {{ t('siterules.list.empty') }}
          </p>
        </DataState>

        <section v-for="entry in rules.byGroup.value" :key="entry.group.group" class="mb-4">
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
            <div v-for="rule in entry.rules" :key="rule.id" class="flex flex-wrap items-center gap-3 p-3">
              <div class="min-w-0 flex-1">
                <p class="text-sm font-medium text-highlighted">{{ rule.name }}</p>
                <p class="truncate font-mono text-[11px] text-muted">{{ rule.hosts.join(', ') || rule.id }}</p>
                <p v-if="!entry.group.enabled" class="mt-1 text-[11px] text-muted">{{ t('siterules.list.group_off') }}</p>
              </div>
              <UBadge :color="stateColor(rule)" variant="subtle" :title="stateTitle(rule)">
                {{ t(`siterules.badge.${stateKey(rule)}`) }}
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
                :label="t('siterules.duplicate')"
                :title="t('siterules.duplicate_hint')"
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
      </template>
    </FormListLayout>
  </div>
</template>
