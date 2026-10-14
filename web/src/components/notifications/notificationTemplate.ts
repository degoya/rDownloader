import type { NotificationPlaceholderTable } from '@/api/notificationPlaceholders'
import type { NotificationEvent } from '@/api/types'
import { OPT_IN_EVENTS } from '@/components/notifications/notificationEvents'

/**
 * A rule's own wording filled in the browser, for the editor's preview (RD-1280-02). The same
 * rules as `crates/rd-notify/src/template.rs`, which fills the real deliveries: `{name}` names a
 * placeholder, `{{` and `}}` write a brace, an unknown name or an unbalanced brace is refused,
 * a placeholder without a value is empty, and a value's control characters become spaces.
 */
export type FillResult =
  | { ok: true, text: string }
  /** `unknown` names the placeholder that does not exist; `null` for a brace out of place. */
  | { ok: false, unknown: string | null }

const CONTROL = /\p{Cc}/gu

export function fillTemplate(
  template: string,
  values: Readonly<Record<string, string>>,
  known: ReadonlySet<string>
): FillResult {
  let out = ''
  for (let index = 0; index < template.length; index++) {
    const char = template.charAt(index)
    const next = template.charAt(index + 1)
    if ((char === '{' || char === '}') && next === char) {
      out += char
      index++
    } else if (char === '}') {
      return { ok: false, unknown: null }
    } else if (char === '{') {
      const close = template.indexOf('}', index + 1)
      const open = template.indexOf('{', index + 1)
      if (close < 0 || (open >= 0 && open < close)) return { ok: false, unknown: null }
      const name = template.slice(index + 1, close).trim()
      if (!name) return { ok: false, unknown: null }
      if (!known.has(name)) return { ok: false, unknown: name }
      out += (values[name] ?? '').replace(CONTROL, ' ')
      index = close
    } else {
      out += char
    }
  }
  return { ok: true, text: out }
}

/** The placeholders the chosen events fill, in the table's order, and whether one of them arrives in bursts. */
export interface OfferedPlaceholders {
  names: string[]
  /**
   * True when a chosen event arrives in bursts (RD-1291-01): worded once per burst, its `{file}`,
   * `{package}` and `{category}` filled only for a burst of one.
   */
  bursts: boolean
}

/**
 * The events a rule naming `events` reacts to. No event chosen means every event but the opt-in
 * ones: the bursts of starts and of added links, and the queue's moments (RD-1290-04).
 */
function chosenEvents(table: NotificationPlaceholderTable, events: readonly NotificationEvent[]) {
  return events.length
    ? table.events.filter(entry => events.includes(entry.event))
    : table.events.filter(entry => entry.takes_template && !OPT_IN_EVENTS.includes(entry.event))
}

/** What a rule naming `events` can fill. */
export function placeholdersFor(table: NotificationPlaceholderTable, events: readonly NotificationEvent[]): OfferedPlaceholders {
  const chosen = chosenEvents(table, events)
  const filled = new Set(chosen.flatMap(entry => entry.placeholders))
  return {
    names: table.placeholders.map(entry => entry.name).filter(name => filled.has(name)),
    bursts: chosen.some(entry => entry.in_bursts)
  }
}

/**
 * The preview's sample values for the placeholders `names`: the table's, or — when every chosen
 * event arrives in bursts — the first one's sample burst, as the service words it (RD-1291-01).
 * Every other placeholder stays empty, as it will.
 */
export function samplesFor(
  table: NotificationPlaceholderTable,
  events: readonly NotificationEvent[],
  names: readonly string[]
): Record<string, string> {
  const samples: Record<string, string> = Object.fromEntries(table.placeholders
    .filter(entry => names.includes(entry.name))
    .map(entry => [entry.name, entry.example]))
  const chosen = chosenEvents(table, events)
  const burst = chosen.length > 0 && chosen.every(entry => entry.in_bursts) ? chosen[0] : undefined
  for (const example of burst?.examples ?? []) {
    if (names.includes(example.name)) samples[example.name] = example.example
  }
  return samples
}
