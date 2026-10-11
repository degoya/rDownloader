/**
 * The capture agent's shortcuts (RD-1180-03) in the interface: what a pressed combination is
 * called, how one is shown on the agent's own platform, and which two commands would collide.
 *
 * The grammar is the service's (`rd_core::Shortcut`): modifiers first in the order CmdOrCtrl,
 * Ctrl, Alt, Shift, Super, then one key, joined by `+`. The service decides what is allowed; this
 * only records and shows, and names a duplicate before a save is refused for it.
 */
import type { CaptureCommand, CaptureShortcuts } from '@/api/types'

/**
 * Every tray command, in the service's order (`CaptureCommand::ALL`): the tray menu's up to
 * `quit`, then the entries that became shortcut-capable with RD-1240-24 and "Restart server"
 * (RD-1240-32).
 */
export const CAPTURE_COMMANDS: readonly CaptureCommand[] = [
  'open',
  'start_all',
  'pause_all',
  'pause_half_hour',
  'pause_hour',
  'clipboard_watch',
  'send_clipboard',
  'game_mode',
  'install_server_update',
  'auto_install',
  'quit',
  'add_all_from_linkgrabber',
  'add_all_from_linkgrabber_paused',
  'install_update',
  'restart_server'
]

export type ShortcutPlatform = 'windows' | 'macos' | 'linux'

/** `KeyboardEvent.code` of the keys with a name, to the grammar's spelling. */
const NAMED_KEYS: Record<string, string> = {
  Space: 'Space',
  Enter: 'Enter',
  NumpadEnter: 'Enter',
  Tab: 'Tab',
  Backspace: 'Backspace',
  Escape: 'Escape',
  Insert: 'Insert',
  Delete: 'Delete',
  Home: 'Home',
  End: 'End',
  PageUp: 'PageUp',
  PageDown: 'PageDown',
  ArrowUp: 'ArrowUp',
  ArrowDown: 'ArrowDown',
  ArrowLeft: 'ArrowLeft',
  ArrowRight: 'ArrowRight',
  Minus: 'Minus',
  Equal: 'Equal',
  BracketLeft: 'BracketLeft',
  BracketRight: 'BracketRight',
  Backslash: 'Backslash',
  Semicolon: 'Semicolon',
  Quote: 'Quote',
  Comma: 'Comma',
  Period: 'Period',
  Slash: 'Slash',
  Backquote: 'Backquote'
}

/**
 * The grammar's key for a physical key, or `null` for a modifier or a key it does not know.
 * The physical key rather than the character, as the agent registers it: `KeyZ` is Z on a German
 * keyboard too, where that key types a Y.
 */
export function keyOfCode(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code)
  if (letter) return letter[1] ?? null
  const digit = /^Digit([0-9])$/.exec(code)
  if (digit) return digit[1] ?? null
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code
  return NAMED_KEYS[code] ?? null
}

type PressedEvent = Pick<KeyboardEvent, 'code' | 'ctrlKey' | 'altKey' | 'shiftKey' | 'metaKey'>

/**
 * The combination a key press makes, or `null` while only modifiers are held. What is held is
 * written as it is — Ctrl as `Ctrl`, the Windows or Cmd key as `Super` — never as `CmdOrCtrl`,
 * which only the defaults use.
 */
export function shortcutFromKeyboardEvent(event: PressedEvent): string | null {
  const key = keyOfCode(event.code)
  if (!key) return null
  const parts: string[] = []
  if (event.ctrlKey) parts.push('Ctrl')
  if (event.altKey) parts.push('Alt')
  if (event.shiftKey) parts.push('Shift')
  if (event.metaKey) parts.push('Super')
  parts.push(key)
  return parts.join('+')
}

interface ParsedShortcut {
  cmdOrCtrl: boolean
  ctrl: boolean
  alt: boolean
  shift: boolean
  superKey: boolean
  key: string
}

const MODIFIERS: Record<string, keyof Omit<ParsedShortcut, 'key'>> = {
  cmdorctrl: 'cmdOrCtrl',
  commandorcontrol: 'cmdOrCtrl',
  ctrl: 'ctrl',
  control: 'ctrl',
  alt: 'alt',
  option: 'alt',
  shift: 'shift',
  super: 'superKey',
  cmd: 'superKey',
  command: 'superKey',
  win: 'superKey',
  meta: 'superKey'
}

/** Reads a stored combination; `null` for one that does not read. */
function parseShortcut(text: string): ParsedShortcut | null {
  const tokens = text.split('+').map(token => token.trim())
  const key = tokens.pop()
  if (!key) return null
  const parsed: ParsedShortcut = { cmdOrCtrl: false, ctrl: false, alt: false, shift: false, superKey: false, key }
  for (const token of tokens) {
    const modifier = MODIFIERS[token.toLowerCase()]
    if (!modifier) return null
    parsed[modifier] = true
  }
  return parsed
}

/** What a combination presses on a Mac or on any other system, with `CmdOrCtrl` resolved. */
function pressedOn(shortcut: ParsedShortcut, mac: boolean): string {
  const ctrl = shortcut.ctrl || (!mac && shortcut.cmdOrCtrl)
  const superKey = shortcut.superKey || (mac && shortcut.cmdOrCtrl)
  return [ctrl, shortcut.alt, shortcut.shift, superKey].map(Number).join('') + shortcut.key.toUpperCase()
}

/**
 * The commands whose combination an earlier command already has, each with that earlier one:
 * the same keys on a Mac or on any other system count, as they do for the service.
 */
export function duplicateCommands(shortcuts: CaptureShortcuts): Map<CaptureCommand, CaptureCommand> {
  const duplicates = new Map<CaptureCommand, CaptureCommand>()
  const seen: [CaptureCommand, ParsedShortcut][] = []
  for (const command of CAPTURE_COMMANDS) {
    const text = shortcuts[command]
    const parsed = text ? parseShortcut(text) : null
    if (!parsed) continue
    const earlier = seen.find(([, other]) =>
      [false, true].some(mac => pressedOn(other, mac) === pressedOn(parsed, mac)))
    if (earlier) duplicates.set(command, earlier[0])
    seen.push([command, parsed])
  }
  return duplicates
}

/**
 * The keys of a combination as the agent's platform names them, one entry per key: `Ctrl`, `Alt`
 * and `Win` on Windows, `Cmd` and `Option` on a Mac. An unreadable text is shown as it is.
 */
export function formatShortcut(text: string, platform: ShortcutPlatform): string[] {
  const parsed = parseShortcut(text)
  if (!parsed) return [text]
  const mac = platform === 'macos'
  const parts: string[] = []
  if (parsed.cmdOrCtrl) parts.push(mac ? 'Cmd' : 'Ctrl')
  if (parsed.ctrl) parts.push('Ctrl')
  if (parsed.alt) parts.push(mac ? 'Option' : 'Alt')
  if (parsed.shift) parts.push('Shift')
  if (parsed.superKey) parts.push(mac ? 'Cmd' : platform === 'windows' ? 'Win' : 'Super')
  parts.push(parsed.key)
  return parts
}

/** The platform of this browser, for an agent that has not reported its own yet. */
export function guessPlatform(agent: string = typeof navigator === 'undefined' ? '' : navigator.userAgent): ShortcutPlatform {
  if (/Mac|iPhone|iPad/.test(agent)) return 'macos'
  if (/Linux|X11/.test(agent) && !/Android/.test(agent)) return 'linux'
  return 'windows'
}
