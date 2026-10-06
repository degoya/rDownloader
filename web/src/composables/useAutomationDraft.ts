import { computed, reactive, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AutomationCondition, AutomationRequest, Category, NotificationTarget } from '@/api/types'
import { useAutomationsStore } from '@/stores/automations'
import { usePostprocessStore } from '@/stores/postprocess'

/** The trigger names the server accepts, taken from the request contract rather than restated. */
export type AutomationTrigger = AutomationRequest['trigger']

/**
 * An action while it is being edited. It differs from the wire type in one way: the id an
 * action points at may still be unset, because the referenced category or notification target
 * may not exist yet. The server takes a UUID and nothing else, so an unset id must never be
 * sent as an empty string — `actionComplete` gates saving instead.
 */
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

/**
 * The automation being edited in `AutomationView.vue` (RD-090-05): the draft, the choices the
 * vocabulary offers for it, and the action rows' editing.
 */
export function useAutomationDraft(categories: Ref<Category[]>, targets: Ref<NotificationTarget[]>) {
  const { t } = useI18n()
  const store = useAutomationsStore()
  const postprocess = usePostprocessStore()

  const draft = reactive({
    name: '',
    enabled: false,
    trigger: 'download_completed' as AutomationTrigger,
    condition: { type: 'always' } as AutomationCondition,
    actions: [] as DraftAction[]
  })

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

  return { draft, triggerOptions, actionKindOptions, canAddAction, scriptItems, canSave, addAction, changeActionKind }
}
