<script setup lang="ts">
/**
 * The automation editor (RD-090-05).
 *
 * Schema-driven: triggers, fields, operators and action kinds all come from
 * `/api/v1/automations/vocabulary`, so the form can only build definitions the server
 * accepts. There is no JSON text area — an automation that has to be hand-written as JSON is
 * one nobody will edit twice.
 */
import { useOverlay } from '@nuxt/ui/composables'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type {
  Automation,
  AutomationAction,
  AutomationCondition,
  AutomationDryRun,
  DownloadPackage,
  NotificationTarget
} from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormActions from '@/components/FormActions.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import AutomationActionCard from '@/components/automation/AutomationActionCard.vue'
import AutomationListItem from '@/components/automation/AutomationListItem.vue'
import AutomationTriggerCard from '@/components/automation/AutomationTriggerCard.vue'
import AutomationVersionsModal from '@/components/automation/AutomationVersionsModal.vue'
import ConditionTree from '@/components/automation/ConditionTree.vue'
import { actionComplete, type AutomationTrigger, toDraft, toWire, useAutomationDraft } from '@/composables/useAutomationDraft'
import { useConfirm } from '@/composables/useConfirm'
import { useCopyName } from '@/composables/useCopyName'
import { useFormBaseline } from '@/composables/useFormBaseline'
import { useFormFocus } from '@/composables/useFormFocus'
import { useUnsavedGuard } from '@/composables/useUnsavedGuard'
import { useAutomationsStore } from '@/stores/automations'
import { useCategories } from '@/stores/categories'
import { usePostprocessStore } from '@/stores/postprocess'
import AreaBackupButtons from '@/components/AreaBackupButtons.vue'
import { describeAction } from '@/utils/automationText'
import { formatMoment } from '@/utils/format'

/** Fields that hold a number; the operator list narrows on these. */
const NUMERIC_FIELDS = ['size_bytes']

const { t } = useI18n()
const store = useAutomationsStore()
const postprocess = usePostprocessStore()
const confirm = useConfirm()
const versionsModal = useOverlay().create(AutomationVersionsModal)
const { categories, fetchCategories } = useCategories()
const packages = ref<DownloadPackage[]>([])
const targets = ref<NotificationTarget[]>([])
const editing = ref<string | null>(null)
const creating = ref(false)
const formElement = ref<HTMLElement | null>(null)
const focusForm = useFormFocus(formElement)
const copyName = useCopyName()
const duplicatingId = ref<string | null>(null)
/** Matches the name check in `crates/rd-automation/src/model.rs`. */
const MAX_AUTOMATION_NAME = 100
const dryRunResult = ref<AutomationDryRun[] | null>(null)
const dryRunPackage = ref<string | null>(null)

const {
  draft,
  triggerOptions,
  actionKindOptions,
  canAddAction,
  scriptItems,
  canSave,
  packageActionOnSchedule,
  request,
  addAction,
  changeActionKind
} = useAutomationDraft(categories, targets)
/** Names of what an action points at, for the dry run's list of what would run. */
const referenceNames = computed(
  () => new Map([...categories.value, ...targets.value].map(item => [item.id, item.name] as [string, string]))
)

const open = computed(() => creating.value || editing.value !== null)
/** An open editor with changes asks before a leave drops them (RD-1120-15). */
const draftBaseline = useFormBaseline(() => draft)
useUnsavedGuard(() => open.value && draftBaseline.dirty.value)

onMounted(async () => {
  await Promise.all([
    store.refresh(),
    store.loadVocabulary(),
    loadReferences(),
    // The same list the package editor and the completion action offer, so a script is picked
    // rather than typed: a name that does not exist fails only when the automation runs.
    postprocess.loadScripts()
  ])
})

// Mounted and released with the view rather than for the whole session, like the
// subscriptions list: the automations are only on screen here, and the shared stream closes
// itself once its last subscriber is gone.
onMounted(() => store.connectEvents())
onUnmounted(() => store.disconnectEvents())

async function loadReferences(): Promise<void> {
  const [, packageResponse, targetResponse] = await Promise.all([
    fetchCategories(),
    api.GET('/api/v1/packages'),
    api.GET('/api/v1/notifications/targets')
  ])
  if (packageResponse.data) packages.value = packageResponse.data
  if (targetResponse.data) targets.value = targetResponse.data
}

function startCreate(): void {
  creating.value = true
  editing.value = null
  dryRunResult.value = null
  Object.assign(draft, {
    name: '',
    enabled: false,
    trigger: (store.vocabulary?.triggers?.[0] ?? 'download_completed') as AutomationTrigger,
    schedule: { kind: 'interval', minutes: 60 },
    condition: { type: 'always' } as AutomationCondition,
    actions: [{ kind: 'pause_package' }]
  })
  draftBaseline.settle()
}

function startEdit(automation: Automation): void {
  creating.value = false
  editing.value = automation.id
  dryRunResult.value = null
  Object.assign(draft, {
    name: automation.name,
    enabled: automation.enabled,
    trigger: (automation.definition?.trigger ?? 'download_completed') as AutomationTrigger,
    schedule: automation.definition?.schedule ?? { kind: 'interval', minutes: 60 },
    condition: (automation.definition?.condition ?? { type: 'always' }) as AutomationCondition,
    actions: ((automation.definition?.actions ?? []) as AutomationAction[]).map(toDraft)
  })
  draftBaseline.settle()
  void focusForm()
}

function cancel(): void {
  creating.value = false
  editing.value = null
  dryRunResult.value = null
}

async function save(): Promise<void> {
  if (!canSave.value) return
  // Only after `canSave` has checked every action carries its id.
  const saved = await store.save(request(), editing.value ?? undefined)
  if (saved) cancel()
}

/**
 * Copies an automation — trigger, condition, actions — under a free name and opens the copy for
 * editing (RD-150-12). It is stored switched off: an identical twin that is on would act a
 * second time on every event its original acts on. The run history stays with the original.
 */
async function duplicate(automation: Automation): Promise<void> {
  duplicatingId.value = automation.id
  const saved = await store.save({
    name: copyName(automation.name, store.automations.map(item => item.name), MAX_AUTOMATION_NAME),
    enabled: false,
    trigger: (automation.definition?.trigger ?? 'download_completed') as AutomationTrigger,
    schedule: automation.definition?.schedule ?? null,
    condition: (automation.definition?.condition ?? { type: 'always' }) as AutomationCondition,
    actions: [...((automation.definition?.actions ?? []) as AutomationAction[])]
  })
  duplicatingId.value = null
  if (saved) startEdit(saved)
}

/**
 * Judges the form as it stands — saved or not, switched on or not (RD-1120-17). Only the stored,
 * enabled automations used to be judged, so a new or switched-off one had nothing to show here.
 */
async function runDryRun(): Promise<void> {
  const { schedule, actions } = request()
  const result = await store.dryRun(draft.trigger, dryRunPackage.value, {
    automation_id: editing.value ?? undefined,
    trigger: draft.trigger,
    schedule,
    condition: draft.condition,
    // An action still missing its id or links is left out rather than sent half-made.
    actions: canSave.value ? actions : draft.actions.filter(actionComplete).map(toWire)
  })
  // A refusal shows in the alert above; an empty list here would read as "nothing matched".
  dryRunResult.value = store.error ? null : result
}

async function removeAutomation(automation: Automation): Promise<void> {
  const confirmed = await confirm({
    title: t('automation.remove.title'),
    description: t('automation.remove.description', { name: automation.name }),
    confirmLabel: t('automation.remove.confirm'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
  if (!confirmed) return
  await store.remove(automation.id)
  if (editing.value === automation.id) cancel()
}

/**
 * Opens the version history (RD-190-22). A restore saves a new version; when the form is open on
 * that automation it is filled again, so the next save does not write the old definition back.
 */
function openVersions(automation: Automation): void {
  versionsModal.open({
    automation,
    categories: categories.value,
    targets: targets.value,
    onRestored: (id: string) => {
      const fresh = store.automations.find(item => item.id === id)
      if (fresh && editing.value === id) startEdit(fresh)
    }
  })
}

function runsOf(id: string) {
  return store.runs.filter(run => run.automation_id === id)
}
</script>

<template>
  <UDashboardPanel id="automation">
    <template #header>
      <UDashboardNavbar :title="t('automation.title')" />
    </template>

    <template #body>
      <SectionHeader level="page" class="mb-4" :eyebrow="t('automation.eyebrow')" :title="t('automation.title')" :description="t('automation.intro')" />

      <FormListLayout :list-title="t('automation.list_title')" :count="store.automations.length">
        <template #form>
          <!-- One card for the column, as on the other form-and-list pages: heading, refusal and
               either the form or, with nothing open, the way in (RD-1110-17). -->
          <UCard as="section">
            <SectionHeader
              class="mb-4"
              :eyebrow="t('automation.eyebrow')"
              :title="editing ? t('automation.edit_title') : t('automation.create_title')"
            />
            <UAlert
              v-if="store.error"
              class="mb-4"
              color="error"
              :description="store.error"
            />
            <form v-if="open" ref="formElement" data-testid="automation-form" @submit.prevent="save">
              <div class="grid gap-4">
                <AutomationTriggerCard
                  v-model:trigger="draft.trigger"
                  v-model:schedule="draft.schedule"
                  :trigger-options="triggerOptions"
                  :package-action-on-schedule="packageActionOnSchedule"
                />
                <UFormField :label="t('automation.name')" required>
                  <UInput v-model="draft.name" required maxlength="100" class="w-full" />
                </UFormField>
                <UFormField :label="t('automation.enabled')" orientation="horizontal">
                  <USwitch v-model="draft.enabled" />
                </UFormField>
              </div>

              <h3 class="mt-5 mb-2 text-sm font-medium text-highlighted">
                {{ t('automation.condition.heading') }}
              </h3>
              <ConditionTree
                v-model="draft.condition"
                :vocabulary="store.vocabulary"
                :depth="0"
                :numeric-fields="NUMERIC_FIELDS"
              />

              <h3 class="mt-5 mb-2 text-sm font-medium text-highlighted">
                {{ t('automation.action.heading') }}
              </h3>
              <p class="mb-2 text-xs text-muted">{{ t('automation.action.at_least_once') }}</p>
              <div class="space-y-2">
                <AutomationActionCard
                  v-for="(action, index) in draft.actions"
                  :key="index"
                  v-model="draft.actions[index]!"
                  :kind-options="actionKindOptions"
                  :script-items="action.kind === 'script' ? scriptItems : []"
                  :categories="categories"
                  :targets="targets"
                  @kind="(kind: string) => changeActionKind(index, kind)"
                  @remove="draft.actions.splice(index, 1)"
                />
              </div>
              <UButton
                class="mt-2"
                icon="i-lucide-plus"
                size="xs"
                color="neutral"
                variant="soft"
                :disabled="!canAddAction"
                :label="t('automation.action.add')"
                @click="addAction"
              />

              <h3 class="mt-5 mb-2 text-sm font-medium text-highlighted">
                {{ t('automation.dry_run.heading') }}
              </h3>
              <p class="mb-2 text-xs text-muted">{{ t('automation.dry_run.help') }}</p>
              <UFieldGroup>
                <USelectMenu
                  :model-value="dryRunPackage ?? undefined"
                  :items="packages"
                  value-key="id"
                  label-key="name"
                  :aria-label="t('automation.dry_run.package')"
                  :placeholder="t('automation.dry_run.package')"
                  class="w-64 max-w-full"
                  @update:model-value="(id: string | undefined) => (dryRunPackage = id ?? null)"
                />
                <UButton
                  icon="i-lucide-flask-conical"
                  color="neutral"
                  variant="outline"
                  :label="t('automation.dry_run.run')"
                  @click="runDryRun"
                />
              </UFieldGroup>
              <ul v-if="dryRunResult" class="mt-3 space-y-1 text-sm">
                <li v-for="match in dryRunResult" :key="match.automation_id" class="text-muted">
                  <span class="font-medium text-highlighted">{{ draft.name.trim() || t('automation.dry_run.draft') }}</span>
                  —
                  {{ match.trigger_matches ? t('automation.dry_run.trigger_yes') : t('automation.dry_run.trigger_no') }},
                  {{ match.condition_matches ? t('automation.dry_run.condition_yes') : t('automation.dry_run.condition_no') }}
                  <template v-if="match.next_run_at">, {{ t('automation.dry_run.next_run', { time: formatMoment(match.next_run_at) }) }}</template>
                  <span v-if="match.actions.length" class="block text-xs" data-testid="automation-dry-run-actions">
                    {{ t('automation.dry_run.would_run') }}:
                    {{ match.actions.map(action => describeAction(action, t, id => referenceNames.get(id))).join(' · ') }}
                  </span>
                </li>
              </ul>

              <FormActions
                class="mt-5"
                :editing="editing !== null"
                :create-label="t('automation.create_title')"
                :loading="store.busy"
                :disabled="!canSave"
                @cancel="cancel"
              />
            </form>
            <!-- Nothing open: the column says what the list on the right is for, and offers the way in. -->
            <template v-else>
              <p class="text-sm leading-6 text-muted">{{ t('automation.pick_or_create') }}</p>
              <UButton class="mt-3" icon="i-lucide-plus" :label="t('automation.create')" @click="startCreate" />
            </template>
          </UCard>
        </template>
        <template #list-actions>
          <AreaBackupButtons area="automations" @imported="store.refresh()" />
        </template>
        <template #list>
          <!-- The list in a card, its empty state too, as on the subscriptions page (RD-1110-17). -->
          <UCard v-if="store.automations.length || !store.error" as="section" :ui="{ body: 'p-0 sm:p-0' }">
            <div v-if="store.automations.length" class="divide-y divide-muted">
              <AutomationListItem
                v-for="automation in store.automations"
                :key="automation.id"
                :automation="automation"
                :editing="editing === automation.id"
                :duplicating="duplicatingId === automation.id"
                :runs="runsOf(automation.id)"
                @toggle="(value: boolean) => store.setEnabled(automation.id, value)"
                @duplicate="duplicate(automation)"
                @versions="openVersions(automation)"
                @edit="startEdit(automation)"
                @remove="removeAutomation(automation)"
              />
            </div>
            <DataState v-else class="p-4 sm:p-6" variant="inline" :loading="store.loading" empty :rows="2">
              <UEmpty :description="t('automation.empty')" />
            </DataState>
          </UCard>
        </template>
      </FormListLayout>
    </template>
  </UDashboardPanel>
</template>
