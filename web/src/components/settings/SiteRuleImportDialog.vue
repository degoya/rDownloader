<script setup lang="ts">
/**
 * What an import of a site-rule file would do, before anything is stored (RD-1230-03).
 *
 * Rules carry no signature any more: a file a colleague exported arrives with each rule's switch
 * as it was there, so this list is where a person decides. Every rule shows its name, its hosts,
 * whether it arrives switched on, and what the import does with it — new, replaces one of the
 * same identifier, already here unchanged, or refused with the reason. A rule that would replace
 * a stored one is replaced only when its own checkbox is ticked; that is the question the job
 * asks, one per rule rather than one for all, because the stored rule may be one somebody wrote
 * here.
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SiteRuleImportPreview, SiteRuleImportedEntry } from '@/api/types'

const props = defineProps<{
  preview: SiteRuleImportPreview | null
  pending: boolean
}>()
const open = defineModel<boolean>('open', { required: true })
const emit = defineEmits<{ import: [replace: string[]] }>()

const { t, te } = useI18n()
/** The identifiers whose stored rule the person agreed to replace. */
const replace = ref<string[]>([])

watch(() => props.preview, () => { replace.value = [] })

const rules = computed(() => props.preview?.rules ?? [])
const fresh = computed(() => rules.value.filter((rule: SiteRuleImportedEntry) => rule.status === 'new').length)
/** Whether the import would write anything at all. */
const writes = computed(() => fresh.value > 0 || replace.value.length > 0)

function statusColor(rule: SiteRuleImportedEntry): 'success' | 'warning' | 'error' | 'neutral' {
  switch (rule.status) {
    case 'new': return 'success'
    case 'replaces': return 'warning'
    case 'refused': return 'error'
    default: return 'neutral'
  }
}

/** Why a rule is refused, in the dialog's words for the two refusals a file brings. */
function reason(rule: SiteRuleImportedEntry): string {
  if (rule.code === 'site_rules.invalid_rule') return t('siterules.import_dialog.invalid')
  if (rule.code === 'site_rules.duplicate_id') return t('siterules.import_dialog.repeated')
  const key = `server.codes.${rule.code ?? ''}`
  return te(key) ? t(key) : (rule.code ?? '')
}

function toggle(id: string, checked: boolean | 'indeterminate'): void {
  replace.value = checked === true
    ? [...new Set([...replace.value, id])]
    : replace.value.filter(entry => entry !== id)
}
</script>

<template>
  <UModal v-model:open="open" :title="t('siterules.import_dialog.title')" :description="t('siterules.import_dialog.description')">
    <template #body>
      <ul class="divide-y divide-muted border border-muted" data-testid="site-rule-import-preview">
        <li v-for="(rule, index) in rules" :key="`${index}:${rule.id}`" class="flex flex-wrap items-start gap-3 p-3" data-import-row>
          <div class="min-w-0 flex-1">
            <p class="text-sm font-medium text-highlighted">{{ rule.name || rule.id || t('siterules.import_dialog.unnamed') }}</p>
            <p class="truncate font-mono text-2xs text-muted">{{ rule.hosts.join(', ') || rule.id }}</p>
            <p v-if="rule.code" class="mt-1 text-2xs text-error">{{ reason(rule) }}</p>
          </div>
          <UBadge size="sm" color="neutral" variant="outline" :icon="rule.enabled ? 'i-lucide-toggle-right' : 'i-lucide-toggle-left'">
            {{ rule.enabled ? t('siterules.import_dialog.on') : t('siterules.import_dialog.off') }}
          </UBadge>
          <UBadge size="sm" :color="statusColor(rule)" variant="subtle">
            {{ t(`siterules.import_dialog.status.${rule.status}`) }}
          </UBadge>
          <UCheckbox
            v-if="rule.status === 'replaces'"
            class="w-full"
            :model-value="replace.includes(rule.id)"
            :label="t('siterules.import_dialog.replace', { name: rule.name || rule.id })"
            @update:model-value="(checked: boolean | 'indeterminate') => toggle(rule.id, checked)"
          />
        </li>
      </ul>
    </template>
    <template #footer>
      <UButton color="neutral" variant="outline" :label="t('common.actions.cancel')" @click="open = false" />
      <UButton
        color="primary"
        icon="i-lucide-file-input"
        :label="t('siterules.import_dialog.confirm')"
        :disabled="!writes"
        :loading="props.pending"
        data-testid="site-rule-import-confirm"
        @click="emit('import', replace)"
      />
    </template>
  </UModal>
</template>
