/**
 * The one-line parameter field of a script subscription (RD-150-08).
 *
 * The line is split the way a shell splits words — at whitespace, with `"…"` and `'…'` keeping
 * an argument with spaces together — and nothing else a shell does: no variables, no escapes,
 * no redirection, and `;`, `&` or `|` are characters like any other. The server never sees the
 * line, only the list, so what the preview shows is exactly what the script receives.
 *
 * A backslash is an ordinary character, so a Windows path needs no doubling. A quote inside an
 * argument is written inside the other kind of quotes (`"it's"`, `'say "hi"'`).
 */
import type { ServerMessage } from '@/i18n/server'

/** Most arguments, and the longest one in characters; the server enforces the same. */
export const MAX_SCRIPT_ARGUMENTS = 32
export const MAX_SCRIPT_ARGUMENT_CHARS = 1024

/** The split line, or `null` when a quote is left open. */
export function splitArguments(line: string): string[] | null {
  const result: string[] = []
  let current = ''
  // Whether an argument has started: `""` is an empty argument, a run of spaces is none.
  let started = false
  let quote: '"' | "'" | null = null
  for (const character of line) {
    if (quote) {
      if (character === quote) quote = null
      else current += character
      continue
    }
    if (character === '"' || character === "'") {
      quote = character
      started = true
    } else if (/\s/.test(character)) {
      if (started) result.push(current)
      current = ''
      started = false
    } else {
      current += character
      started = true
    }
  }
  if (quote) return null
  if (started) result.push(current)
  return result
}

/** Writes a list back as a line that `splitArguments` turns into the same list. */
export function joinArguments(values: readonly string[]): string {
  return values.map(quoteArgument).join(' ')
}

function quoteArgument(value: string): string {
  if (/^[^\s"']+$/.test(value)) return value
  if (!value.includes("'")) return `'${value}'`
  if (!value.includes('"')) return `"${value}"`
  // Both kinds of quote: single-quoted runs, each `'` itself inside double quotes.
  return value.split("'").map(part => `'${part}'`).join(`"'"`)
}

/** The first limit the list breaks, as the code the server would answer with. */
export function argumentsProblem(values: readonly string[]): ServerMessage | null {
  if (values.length > MAX_SCRIPT_ARGUMENTS) {
    return { code: 'subscription.script_arguments_too_many', params: { maximum: String(MAX_SCRIPT_ARGUMENTS) } }
  }
  for (const [index, value] of values.entries()) {
    const position = String(index + 1)
    // Characters, not UTF-16 units: the server counts what a person counts.
    if ([...value].length > MAX_SCRIPT_ARGUMENT_CHARS) {
      return {
        code: 'subscription.script_argument_too_long',
        params: { position, maximum: String(MAX_SCRIPT_ARGUMENT_CHARS) }
      }
    }
    if (/[\0\r\n]/.test(value)) {
      return { code: 'subscription.script_argument_invalid', params: { position } }
    }
  }
  return null
}
