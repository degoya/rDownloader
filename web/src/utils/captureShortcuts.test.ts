/**
 * The capture agent's shortcuts in the interface (RD-1180-03): a pressed combination becomes the
 * service's spelling, a stored one is shown with the agent's own key names, and two commands
 * with one combination are named before the service refuses the save.
 */
import { describe, expect, it } from 'vitest'

import type { CaptureShortcuts } from '@/api/types'

import {
  CAPTURE_COMMANDS,
  duplicateCommands,
  formatShortcut,
  guessPlatform,
  keyOfCode,
  shortcutFromKeyboardEvent
} from './captureShortcuts'

function press(code: string, held: Partial<Record<'ctrlKey' | 'altKey' | 'shiftKey' | 'metaKey', boolean>> = {}) {
  return { code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...held }
}

describe('captureShortcuts', () => {
  it('names the physical key, not the character it types', () => {
    expect(keyOfCode('KeyZ')).toBe('Z')
    expect(keyOfCode('Digit7')).toBe('7')
    expect(keyOfCode('F24')).toBe('F24')
    expect(keyOfCode('F25')).toBeNull()
    expect(keyOfCode('NumpadEnter')).toBe('Enter')
    expect(keyOfCode('ArrowUp')).toBe('ArrowUp')
    expect(keyOfCode('ControlLeft')).toBeNull()
    expect(keyOfCode('IntlBackslash')).toBeNull()
  })

  it('records a combination in the service order and waits while only modifiers are held', () => {
    expect(shortcutFromKeyboardEvent(press('KeyV', { ctrlKey: true, altKey: true }))).toBe('Ctrl+Alt+V')
    expect(shortcutFromKeyboardEvent(press('KeyK', { metaKey: true, shiftKey: true, altKey: true }))).toBe('Alt+Shift+Super+K')
    expect(shortcutFromKeyboardEvent(press('AltLeft', { altKey: true }))).toBeNull()
  })

  it('shows a combination with the key names of the agent’s platform', () => {
    expect(formatShortcut('CmdOrCtrl+Alt+V', 'windows')).toEqual(['Ctrl', 'Alt', 'V'])
    expect(formatShortcut('CmdOrCtrl+Alt+V', 'macos')).toEqual(['Cmd', 'Option', 'V'])
    expect(formatShortcut('Ctrl+Super+F13', 'windows')).toEqual(['Ctrl', 'Win', 'F13'])
    expect(formatShortcut('Ctrl+Super+F13', 'linux')).toEqual(['Ctrl', 'Super', 'F13'])
    expect(formatShortcut('nonsense+', 'linux')).toEqual(['nonsense+'])
  })

  it('names a command whose keys an earlier one presses on either platform', () => {
    const shortcuts: CaptureShortcuts = {
      open: 'CmdOrCtrl+Alt+O',
      send_clipboard: 'CmdOrCtrl+Alt+V',
      // The same keys as send_clipboard on Windows and Linux.
      quit: 'Ctrl+Alt+V'
    }
    expect([...duplicateCommands(shortcuts)]).toEqual([['quit', 'send_clipboard']])
    // Cmd+Option+V on a Mac: CmdOrCtrl is Cmd there.
    expect([...duplicateCommands({ send_clipboard: 'CmdOrCtrl+Alt+V', quit: 'Super+Alt+V' })])
      .toEqual([['quit', 'send_clipboard']])
    expect(duplicateCommands({ open: 'CmdOrCtrl+Alt+O', quit: null }).size).toBe(0)
  })

  it('lists every tray command, in the order of the service', () => {
    expect(CAPTURE_COMMANDS[0]).toBe('open')
    expect(CAPTURE_COMMANDS.indexOf('quit')).toBe(10)
    expect(CAPTURE_COMMANDS.slice(11)).toEqual([
      'add_all_from_linkgrabber',
      'add_all_from_linkgrabber_paused',
      'install_update',
      'restart_server'
    ])
    expect(new Set(CAPTURE_COMMANDS).size).toBe(15)
    expect(CAPTURE_COMMANDS).toHaveLength(15)
  })

  it('guesses the platform of a browser whose agent has not reported yet', () => {
    expect(guessPlatform('Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)')).toBe('macos')
    expect(guessPlatform('Mozilla/5.0 (X11; Linux x86_64)')).toBe('linux')
    expect(guessPlatform('Mozilla/5.0 (Windows NT 10.0; Win64; x64)')).toBe('windows')
  })
})
