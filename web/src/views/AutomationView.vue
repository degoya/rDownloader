<script setup lang="ts">
/**
 * The automation editor (RD-090-05).
 *
 * Schema-driven: triggers, fields, operators and action kinds all come from
 * `/api/v1/automations/vocabulary`, so the form can only build definitions the server
 * accepts. There is no JSON text area — an automation that has to be hand-written as JSON is
 * one nobody will edit twice.
 */
import { computed, onMounted, onUnmounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type {
  Automation,
  AutomationAction,
  AutomationRequest,
  AutomationCondition,
  AutomationDryRun,
  Category,
  DownloadPackage,
  NotificationTarget
} from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import ConditionTree from '@/components/automation/ConditionTree.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useFormFocus } from '@/composables/useFormFocus'
import { useAutomationsStore } from '@/stores/automations'
import { usePostprocessStore } from '@/stores/postprocess'
import AreaBackupButtons from '@/components/AreaBackupButtons.vue'
import { formatMoment } from '@/utils/format'

/** Fields that hold a number; the operator list narrows on these. */
const NUMERIC_FIELDS = ['size_bytes']

/**
 * An action while it is being edited. It differs from the wire type in one way: the id an
 * action points at may still be unset, because the referenced category or notification target
 * may not exist yet. The server takes a UUID and nothing else, so an unset id must never be
 * sent as an empty string — `actionComplete` gates saving instead.
 */
/** The trigger names the server accepts, taken from the request contract rather than restated. */
type AutomationTrigger = AutomationRequest['trigger']

type DraftAction = { kind: string } & Partial<{
  name: string
  category_id: string
  target_id: string
}>

/** Whether an action names everything the server needs to accept it. */
function actionComplete(action: DraftAction): boolean {
  if (action.kind === 'script') return Boolean(action.name?.trim())
  if (action.kind === 'set_category') return Boolean(action.category_id)
  if (action.kind === 'webhook') return Boolean(action.target_id)
  return true
}

const { t } = useI18n()
const store = useAutomationsStore()
const postprocess = usePostprocessStore()
const confirm = useConfirm()
const categories = ref<Category[]>([])
const packages = ref<DownloadPackage[]>([])
const targets = ref<NotificationTarget[]>([])
const editing = ref<string | null>(null)
const creating = ref(false)
const formElement = ref<HTMLElement | null>(null)
const focusForm = useFormFocus(formElement)
const dryRunResult = ref<AutomationDryRun[] | null>(null)
const dryRunPackage = ref<string | null>(null)

const draft = reactive({
  name: '',
  enabled: false,
  trigger: 'download_completed' as AutomationTrigger,
  condition: { type: 'always' } as AutomationCondition,
  actions: [] as DraftAction[]
})

const open = computed(() => creating.value || editing.value !== null)
const triggerOptions = computed(() =>
  (store.vocabulary?.triggers ?? []).map(trigger => ({
    value: trigger,
    label: t(`automation.trigger.${trigger}`)
  }))
)
const actionKindOptions = computed(() =>
  (store.vocabulary?.action_kinds ?? []).map(kind => ({
    value: kind,
    label: t(`automation.action.${kind}`)
  }))
)
const canAddAction = computed(
  () => draft.actions.length < (store.vocabulary?.max_actions ?? 10)
)

/** Script names to choose from, keeping one already saved that is no longer in the folder. */
const scriptItems = computed(() => {
  const saved = draft.actions
    .filter(action => action.kind === 'script')
    .map(action => action.name)
    .filter((name): name is string => typeof name === 'string' && name.length > 0)
    .filter(name => !postprocess.scripts.includes(name))
  return [...new Set([...saved, ...postprocess.scripts])].map(name => ({ label: name, value: name }))
})

const canSave = computed(
  () =>
    Boolean(draft.name.trim())
    && draft.actions.length > 0
    && draft.actions.every(actionComplete)
)

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

function addAction(): void {
  if (!canAddAction.value) return
  draft.actions.push({ kind: 'pause_package' })
}

function changeActionKind(index: number, kind: string): void {
  const action: DraftAction = { kind }
  // Seeded with the first script there is, for the same reason the ids are: a form that
  // opens on an unusable value invites a save that cannot work.
  if (kind === 'script') action.name = postprocess.scripts[0] ?? ''
  // Seeded with the first available id when there is one. When there is none the field stays
  // absent rather than empty, so the save button reports it instead of the JSON parser.
  if (kind === 'set_category' && categories.value[0]) action.category_id = categories.value[0].id
  if (kind === 'webhook' && targets.value[0]) action.target_id = targets.value[0].id
  draft.actions[index] = action
}

async function save(): Promise<void> {
  if (!canSave.value) return
  // The one cast, and only after `canSave` has checked every action carries its id.
  const ok = await store.save(
    {
      name: draft.name,
      enabled: draft.enabled,
      trigger: draft.trigger,
      condition: draft.condition,
      actions: draft.actions as AutomationAction[]
    },
    editing.value ?? undefined
  )
  if (ok) cancel()
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

function runsOf(id: string) {
  return store.runs.filter(run => run.automation_id === id)
}
</script>

<template>
  <UDashboardPanel id="automation">
    <template #header>
      <UDashboardNavbar :title="t('automation.title')">
        <template #right>
          <UButton icon="i-lucide-plus" :label="t('automation.create')" @click="startCreate" />
        </template>
      </UDashboardNavbar>
    </template>

    <template #body>
      <UAlert
        v-if="store.error"
        class="mb-4"
        color="error"
        variant="subtle"
        :description="store.error"
      />

      <p class="mb-4 text-sm leading-6 text-muted">{{ t('automation.intro') }}</p>

      <FormListLayout :list-title="t('automation.list_title')" :count="store.automations.length">
        <template #form>
          <h2 class="mb-3 text-sm font-semibold text-highlighted">
            {{ editing ? t('automation.edit_title') : t('automation.create_title') }}
          </h2>
          <section v-if="open" ref="formElement" class="border border-muted bg-default p-5">
            <div class="grid gap-4">
              <UFormField :label="t('automation.name')" required>
                <UInput v-model="draft.name" maxlength="100" class="w-full" />
              </UFormField>
              <UFormField :label="t('automation.trigger_label')" :help="t('automation.trigger_help')">
                <USelectMenu
                  :model-value="draft.trigger"
                  :items="triggerOptions"
                  value-key="value"
                  class="w-full"
                  @update:model-value="(value: AutomationTrigger) => (draft.trigger = value)"
                />
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

            <div class="mt-5 flex items-center gap-2">
              <USwitch v-model="draft.enabled" :label="t('automation.enabled')" />
              <div class="ml-auto flex gap-2">
                <UButton color="neutral" variant="ghost" :label="t('common.actions.cancel')" @click="cancel" />
                <UButton
                  icon="i-lucide-save"
                  :label="t('common.actions.save')"
                  :loading="store.busy"
                  :disabled="!canSave"
                  @click="save"
                />
              </div>
            </div>
          </section>
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
            <article v-for="automation in store.automations" :key="automation.id" class="p-4" :class="editing === automation.id ? 'border-l-2 border-l-primary' : ''">
              <div class="flex flex-wrap items-center gap-3">
                <span class="size-2" :class="automation.enabled ? 'bg-success' : 'bg-muted'" />
                <div class="min-w-0 flex-1">
                  <p class="truncate text-sm font-medium text-highlighted">{{ automation.name }}</p>
                  <p class="text-xs text-muted">
                    {{ t(`automation.trigger.${automation.definition?.trigger ?? 'download_completed'}`) }}
                    · {{ t('automation.version', { version: automation.version }) }}
                    · {{ t('automation.action_count', { count: automation.definition?.actions?.length ?? 0 }) }}
                  </p>
                </div>
                <UBadge v-if="editing === automation.id" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
                <USwitch
                  :model-value="automation.enabled"
                  :aria-label="t('automation.enabled')"
                  @update:model-value="(value: boolean) => store.setEnabled(automation.id, value)"
                />
                <UButton
                  icon="i-lucide-pencil"
                  size="xs"
                  color="neutral"
                  variant="ghost"
                  :aria-label="t('common.actions.edit')"
                  @click="startEdit(automation)"
                />
                <UButton
                  icon="i-lucide-trash-2"
                  size="xs"
                  color="error"
                  variant="ghost"
                  :aria-label="t('automation.remove.confirm')"
                  @click="removeAutomation(automation)"
                />
              </div>
              <ul v-if="runsOf(automation.id).length" class="mt-3 space-y-1">
                <li
                  v-for="run in runsOf(automation.id).slice(0, 5)"
                  :key="run.id"
                  class="flex flex-wrap items-center gap-2 text-xs text-muted"
                >
                  <span class="font-medium">{{ t(`automation.run_state.${run.state}`) }}</span>
                  <span class="numeric">{{ formatMoment(run.started_at) }}</span>
                  <span v-if="run.message" class="truncate">{{ run.message }}</span>
                </li>
              </ul>
              <p v-else class="mt-3 text-xs text-muted">{{ t('automation.no_runs') }}</p>
            </article>
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
