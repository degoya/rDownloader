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
  Category,
  DownloadPackage,
  NotificationTarget
} from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormActions from '@/components/FormActions.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import AutomationListItem from '@/components/automation/AutomationListItem.vue'
import AutomationVersionsModal from '@/components/automation/AutomationVersionsModal.vue'
import ConditionTree from '@/components/automation/ConditionTree.vue'
import { type AutomationTrigger, useAutomationDraft } from '@/composables/useAutomationDraft'
import { useConfirm } from '@/composables/useConfirm'
import { useCopyName } from '@/composables/useCopyName'
import { useFormFocus } from '@/composables/useFormFocus'
import { useAutomationsStore } from '@/stores/automations'
import { usePostprocessStore } from '@/stores/postprocess'
import AreaBackupButtons from '@/components/AreaBackupButtons.vue'

/** Fields that hold a number; the operator list narrows on these. */
const NUMERIC_FIELDS = ['size_bytes']

const { t } = useI18n()
const store = useAutomationsStore()
const postprocess = usePostprocessStore()
const confirm = useConfirm()
const versionsModal = useOverlay().create(AutomationVersionsModal)
const categories = ref<Category[]>([])
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

const { draft, triggerOptions, actionKindOptions, canAddAction, scriptItems, canSave, addAction, changeActionKind } =
  useAutomationDraft(categories, targets)

const open = computed(() => creating.value || editing.value !== null)

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
  const [categoryResponse, packageResponse, targetResponse] = await Promise.all([
    api.GET('/api/v1/categories'),
    api.GET('/api/v1/packages'),
    api.GET('/api/v1/notifications/targets')
  ])
  if (categoryResponse.data) categories.value = categoryResponse.data
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
    condition: { type: 'always' } as AutomationCondition,
    actions: [{ kind: 'pause_package' } as AutomationAction]
  })
}

function startEdit(automation: Automation): void {
  creating.value = false
  editing.value = automation.id
  dryRunResult.value = null
  Object.assign(draft, {
    name: automation.name,
    enabled: automation.enabled,
    trigger: (automation.definition?.trigger ?? 'download_completed') as AutomationTrigger,
    condition: (automation.definition?.condition ?? { type: 'always' }) as AutomationCondition,
    actions: [...((automation.definition?.actions ?? []) as AutomationAction[])]
  })
  void focusForm()
}

function cancel(): void {
  creating.value = false
  editing.value = null
  dryRunResult.value = null
}

async function save(): Promise<void> {
  if (!canSave.value) return
  // The one cast, and only after `canSave` has checked every action carries its id.
  const saved = await store.save(
    {
      name: draft.name,
      enabled: draft.enabled,
      trigger: draft.trigger,
      condition: draft.condition,
      actions: draft.actions as AutomationAction[]
    },
    editing.value ?? undefined
  )
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
    condition: (automation.definition?.condition ?? { type: 'always' }) as AutomationCondition,
    actions: [...((automation.definition?.actions ?? []) as AutomationAction[])]
  })
  duplicatingId.value = null
  if (saved) startEdit(saved)
}

async function runDryRun(): Promise<void> {
  dryRunResult.value = await store.dryRun(draft.trigger, dryRunPackage.value)
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
          <SectionHeader
            class="mb-3"
            :eyebrow="t('automation.eyebrow')"
            :title="editing ? t('automation.edit_title') : t('automation.create_title')"
          />
          <UAlert
            v-if="store.error"
            class="mb-3"
            color="error"
            variant="subtle"
            :description="store.error"
          />
          <form v-if="open" ref="formElement" class="border border-muted bg-default p-5" data-testid="automation-form" @submit.prevent="save">
            <div class="grid gap-4">
              <!-- The trigger is the automation's kind, so it comes first, before its name. -->
              <UFormField :label="t('automation.trigger_label')" :description="t('automation.trigger_help')">
                <USelectMenu
                  :model-value="draft.trigger"
                  :items="triggerOptions"
                  value-key="value"
                  class="w-full"
                  @update:model-value="(value: AutomationTrigger) => (draft.trigger = value)"
                />
              </UFormField>
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
              <div
                v-for="(action, index) in draft.actions"
                :key="index"
                class="flex flex-wrap items-center gap-2 border border-muted p-3"
              >
                <USelectMenu
                  :model-value="action.kind"
                  :items="actionKindOptions"
                  value-key="value"
                  :aria-label="t('automation.action.kind')"
                  class="w-52"
                  @update:model-value="(value: string) => changeActionKind(index, value)"
                />
                <USelect
                  v-if="action.kind === 'script' && scriptItems.length"
                  :model-value="action.name"
                  :items="scriptItems"
                  value-key="value"
                  :aria-label="t('automation.action.script_name')"
                  class="w-56 font-mono"
                  @update:model-value="(name: string) => (draft.actions[index]!.name = name)"
                />
                <p v-else-if="action.kind === 'script'" class="self-center text-xs text-error">
                  {{ t('automation.action.no_scripts') }}
                </p>
                <USelectMenu
                  v-if="action.kind === 'set_category'"
                  :model-value="action.category_id"
                  :items="categories"
                  value-key="id"
                  label-key="name"
                  :aria-label="t('automation.action.category')"
                  :placeholder="t('automation.action.category')"
                  class="w-56"
                  @update:model-value="(id: string) => (draft.actions[index]!.category_id = id)"
                />
                <USelectMenu
                  v-if="action.kind === 'webhook'"
                  :model-value="action.target_id"
                  :items="targets"
                  value-key="id"
                  label-key="name"
                  :filter-fields="['name', 'endpoint']"
                  :aria-label="t('automation.action.target')"
                  :placeholder="t('automation.action.target')"
                  class="w-56"
                  @update:model-value="(id: string) => (draft.actions[index]!.target_id = id)"
                />
                <p v-if="action.kind === 'webhook' && !targets.length" class="self-center text-xs text-error">
                  {{ t('automation.action.no_targets') }}
                </p>
                <UButton
                  icon="i-lucide-x"
                  size="xs"
                  color="error"
                  variant="ghost"
                  :aria-label="t('automation.action.remove')"
                  @click="draft.actions.splice(index, 1)"
                />
              </div>
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
            <div class="flex flex-wrap items-end gap-2">
              <USelectMenu
                :model-value="dryRunPackage ?? undefined"
                :items="packages"
                value-key="id"
                label-key="name"
                :aria-label="t('automation.dry_run.package')"
                :placeholder="t('automation.dry_run.package')"
                class="w-64"
                @update:model-value="(id: string | undefined) => (dryRunPackage = id ?? null)"
              />
              <UButton
                icon="i-lucide-flask-conical"
                color="neutral"
                variant="soft"
                :label="t('automation.dry_run.run')"
                @click="runDryRun"
              />
            </div>
            <ul v-if="dryRunResult" class="mt-3 space-y-1 text-sm">
              <li v-for="match in dryRunResult" :key="match.automation_id" class="text-muted">
                <span class="font-medium text-highlighted">
                  {{ store.automations.find(item => item.id === match.automation_id)?.name ?? match.automation_id }}
                </span>
                —
                {{ match.trigger_matches ? t('automation.dry_run.trigger_yes') : t('automation.dry_run.trigger_no') }},
                {{ match.condition_matches ? t('automation.dry_run.condition_yes') : t('automation.dry_run.condition_no') }}
              </li>
              <li v-if="!dryRunResult.length" class="text-muted">{{ t('automation.dry_run.none') }}</li>
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
          <section v-else class="border border-dashed border-muted p-5">
            <p class="text-sm leading-6 text-muted">{{ t('automation.pick_or_create') }}</p>
            <UButton class="mt-3" icon="i-lucide-plus" :label="t('automation.create')" @click="startCreate" />
          </section>
        </template>
        <template #list-actions>
          <AreaBackupButtons area="automations" @imported="store.refresh()" />
        </template>
        <template #list>
          <div v-if="store.automations.length" class="divide-y divide-muted border border-muted">
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
          <DataState v-else :loading="store.loading" :empty="!store.error" :rows="2">
            <p class="border border-dashed border-muted p-8 text-center text-sm text-muted">
              {{ t('automation.empty') }}
            </p>
          </DataState>
        </template>
      </FormListLayout>
    </template>
  </UDashboardPanel>
</template>
