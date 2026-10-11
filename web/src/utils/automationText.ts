import type { AutomationAction, AutomationCondition, AutomationSchedule, AutomationVersion } from '@/api/types'

type Translate = (key: string, params?: Record<string, unknown>) => string

/**
 * An automation's condition as one line of text, for the version history (RD-190-22).
 *
 * The editor draws a condition as a tree of fields; the history only has to say what a version
 * checked, so it reads the same tree as words in reading order: "All of (Name contains "x";
 * Not (Extension equals "nfo"))".
 */
export function describeCondition(node: AutomationCondition, t: Translate): string {
  switch (node.type) {
    case 'always':
      return t('automation.condition.always')
    case 'predicate': {
      const { field, operator, value } = node.predicate
      return `${t(`automation.field.${field}`)} ${t(`automation.operator.${operator}`)} "${value}"`
    }
    case 'all':
    case 'any':
      return `${t(`automation.condition.${node.type}`)} (${node.nodes.map(child => describeCondition(child, t)).join('; ')})`
    case 'not':
      return `${t('automation.condition.not')} (${describeCondition(node.node, t)})`
  }
}

/** One action as text: its kind and, where it points somewhere, the name of what it points at. */
export function describeAction(action: AutomationAction, t: Translate, names: (id: string) => string | undefined): string {
  const kind = t(`automation.action.${action.kind}`)
  if (action.kind === 'script') return `${kind}: ${action.name}`
  if (action.kind === 'set_category') return `${kind}: ${names(action.category_id) ?? action.category_id}`
  if (action.kind === 'webhook') return `${kind}: ${names(action.target_id) ?? action.target_id}`
  if (action.kind === 'set_priority') return `${kind}: ${t(`automation.action.priority_value.${action.priority}`)}`
  if (action.kind === 'notify') return `${kind}: ${names(action.target_id) ?? action.target_id} "${action.message}"`
  if (action.kind === 'add_links') {
    const destination = t(`automation.action.destination_value.${action.destination ?? 'link_grabber'}`)
    return `${kind}: ${t('automation.action.link_count', { count: action.links.length })} → ${destination}`
  }
  return kind
}

/** A time trigger's schedule as text: "every 60 minutes", or the cron line itself. */
export function describeSchedule(schedule: AutomationSchedule, t: Translate): string {
  if (schedule.kind === 'interval') return t('automation.schedule.every_minutes', { minutes: schedule.minutes })
  return schedule.expression
}

/** The trigger as text; a time trigger says when (RD-1240-10). */
export function describeTrigger(trigger: string, schedule: AutomationSchedule | null | undefined, t: Translate): string {
  const name = t(`automation.trigger.${trigger}`)
  return trigger === 'schedule' && schedule ? `${name}: ${describeSchedule(schedule, t)}` : name
}

/** Which parts of `version` differ from the version saved before it; nothing for the first. */
export function changedParts(version: AutomationVersion, previous: AutomationVersion | undefined): Set<'trigger' | 'condition' | 'actions'> {
  const changed = new Set<'trigger' | 'condition' | 'actions'>()
  if (!previous) return changed
  if (version.trigger !== previous.trigger || JSON.stringify(version.schedule ?? null) !== JSON.stringify(previous.schedule ?? null)) {
    changed.add('trigger')
  }
  if (JSON.stringify(version.condition) !== JSON.stringify(previous.condition)) changed.add('condition')
  if (JSON.stringify(version.actions) !== JSON.stringify(previous.actions)) changed.add('actions')
  return changed
}
