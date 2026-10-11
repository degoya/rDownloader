import { computed, reactive, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type {
  AutomationAction,
  AutomationCondition,
  AutomationRequest,
  AutomationSchedule,
  Category,
  NotificationTarget
} from '@/api/types'
import { useAutomationsStore } from '@/stores/automations'
import { usePostprocessStore } from '@/stores/postprocess'

/** The trigger names the server accepts, taken from the request contract rather than restated. */
export type AutomationTrigger = AutomationRequest['trigger']

/** Matches the limits in `crates/rd-automation/src/model.rs` and `schedule.rs`. */
export const MAX_ACTION_LINKS = 50
export const MAX_NOTIFY_MESSAGE = 500
export const MAX_INTERVAL_MINUTES = 1440

/** Actions that work on the run's package; a time trigger names none (RD-1240-10). */
export const PACKAGE_ACTIONS = ['set_category', 'pause_package', 'resume_package', 'set_priority', 'extract_package']

/**
 * An action while it is being edited. It differs from the wire type in two ways: the id an
 * action points at may still be unset, because the referenced category or notification target
 * may not exist yet, and an `add_links` action holds its text area as typed. The server takes a
 * UUID and nothing else, so an unset id must never be sent as an empty string — `actionComplete`
 * gates saving instead, and `toWire` turns the text into the link list.
 */
export type DraftAction = { kind: string } & Partial<{
  name: string
  category_id: string
  target_id: string
  priority: 'low' | 'normal' | 'high'
  message: string
  links_text: string
  destination: 'link_grabber' | 'downloads'
}>

/** The non-empty lines of an `add_links` text area. */
export function linksOf(text: string | undefined): string[] {
  return (text ?? '').split('\n').map(line => line.trim()).filter(line => line.length > 0)
}

/** Whether an action names everything the server needs to accept it. */
export function actionComplete(action: DraftAction): boolean {
  if (action.kind === 'script') return Boolean(action.name?.trim())
  if (action.kind === 'set_category') return Boolean(action.category_id)
  if (action.kind === 'webhook') return Boolean(action.target_id)
  if (action.kind === 'set_priority') return Boolean(action.priority)
  if (action.kind === 'notify') {
    const length = action.message?.trim().length ?? 0
    return Boolean(action.target_id) && length > 0 && length <= MAX_NOTIFY_MESSAGE
  }
  if (action.kind === 'add_links') {
    const count = linksOf(action.links_text).length
    return count > 0 && count <= MAX_ACTION_LINKS
  }
  return true
}

/** Whether a schedule names a time the server can read; the server checks a cron line itself. */
function scheduleComplete(schedule: AutomationSchedule): boolean {
  if (schedule.kind === 'interval') {
    return Number.isInteger(schedule.minutes) && schedule.minutes >= 1 && schedule.minutes <= MAX_INTERVAL_MINUTES
  }
  return schedule.expression.trim().length > 0
}

/** A stored action as the editor holds it. */
export function toDraft(action: AutomationAction): DraftAction {
  if (action.kind === 'add_links') {
    return { kind: action.kind, links_text: action.links.join('\n'), destination: action.destination ?? 'link_grabber' }
  }
  return { ...action }
}

/** A draft action as the server takes it; only called once `actionComplete` holds. */
export function toWire(action: DraftAction): AutomationAction {
  if (action.kind === 'add_links') {
    return { kind: 'add_links', links: linksOf(action.links_text), destination: action.destination ?? 'link_grabber' }
  }
  return { ...action } as AutomationAction
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
    schedule: { kind: 'interval', minutes: 60 } as AutomationSchedule,
    condition: { type: 'always' } as AutomationCondition,
    actions: [] as DraftAction[]
  })

  const timed = computed(() => draft.trigger === 'schedule')

  const triggerOptions = computed(() =>
    (store.vocabulary?.triggers ?? []).map(trigger => ({
      value: trigger,
      label: t(`automation.trigger.${trigger}`)
    }))
  )
  /** A time trigger offers no package action: it has no package to act on. */
  const actionKindOptions = computed(() =>
    (store.vocabulary?.action_kinds ?? [])
      .filter(kind => !timed.value || !PACKAGE_ACTIONS.includes(kind))
      .map(kind => ({
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

  /** A time trigger holding a package action from before the trigger changed. */
  const packageActionOnSchedule = computed(
    () => timed.value && draft.actions.some(action => PACKAGE_ACTIONS.includes(action.kind))
  )

  const canSave = computed(
    () =>
      Boolean(draft.name.trim())
      && draft.actions.length > 0
      && draft.actions.every(actionComplete)
      && (!timed.value || (scheduleComplete(draft.schedule) && !packageActionOnSchedule.value))
  )

  /** The draft as the server takes it: the schedule only where the trigger reads it. */
  function request(): AutomationRequest {
    return {
      name: draft.name,
      enabled: draft.enabled,
      trigger: draft.trigger,
      schedule: timed.value ? draft.schedule : null,
      condition: draft.condition,
      actions: draft.actions.map(toWire)
    }
  }

  function addAction(): void {
    if (!canAddAction.value) return
    draft.actions.push({ kind: timed.value ? 'start_queue' : 'pause_package' })
  }

  function changeActionKind(index: number, kind: string): void {
    const action: DraftAction = { kind }
    // Seeded with the first script there is, for the same reason the ids are: a form that
    // opens on an unusable value invites a save that cannot work.
    if (kind === 'script') action.name = postprocess.scripts[0] ?? ''
    // Seeded with the first available id when there is one. When there is none the field stays
    // absent rather than empty, so the save button reports it instead of the JSON parser.
    if (kind === 'set_category' && categories.value[0]) action.category_id = categories.value[0].id
    if ((kind === 'webhook' || kind === 'notify') && targets.value[0]) action.target_id = targets.value[0].id
    if (kind === 'set_priority') action.priority = 'high'
    if (kind === 'notify') action.message = ''
    if (kind === 'add_links') Object.assign(action, { links_text: '', destination: 'link_grabber' })
    draft.actions[index] = action
  }

  return {
    draft,
    timed,
    triggerOptions,
    actionKindOptions,
    canAddAction,
    scriptItems,
    canSave,
    packageActionOnSchedule,
    request,
    addAction,
    changeActionKind
  }
}
