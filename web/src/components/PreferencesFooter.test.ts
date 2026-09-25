/**
 * The language and theme choice at the bottom of the sidebar (RD-110-32).
 *
 * Expanded, two selects; collapsed, one gear that opens a menu holding both. The Nuxt UI
 * components cannot be imported here (see `ControlRoomLayout.test.ts`), so the menu stub
 * renders every item as a button that calls the item's `onSelect` — what the real
 * `UDropdownMenu` does when an item is chosen. What is covered is this component's own
 * wiring: the face and name of the gear, the items it offers and where a choice lands.
 */
import { fireEvent, render, screen } from '@testing-library/vue'
import axe from 'axe-core'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: undefined })), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn()
}))

import PreferencesFooter from './PreferencesFooter.vue'
import { useTheme } from '@/composables/useTheme'
import { i18n, SUPPORTED_LOCALES } from '@/i18n'

const stubs = {
  UButton: { template: '<button type="button" v-bind="$attrs" />' },
  UDropdownMenu: {
    props: ['items'],
    template: `<div><slot />
      <div v-for="(group, index) in items" :key="index" data-testid="menu-group">
        <button v-for="item in group" :key="item.label" type="button" :data-checked="String(item.checked)" @click="item.onSelect()">{{ item.label }}</button>
      </div>
    </div>`
  },
  USelect: { props: ['modelValue', 'items', 'size', 'icon'], template: '<select v-bind="$attrs" />' }
}

function mount(collapsed: boolean) {
  return render(PreferencesFooter, { props: { collapsed }, global: { plugins: [i18n], stubs } })
}

async function violations(container: Element): Promise<string> {
  const results = await axe.run(container, { rules: { region: { enabled: false } } })
  return results.violations
    .map(violation => `${violation.id}: ${violation.help} (${violation.nodes.length} node(s))`)
    .join('\n')
}

const { theme } = useTheme()

beforeEach(() => {
  i18n.global.locale.value = 'en'
  theme.value = 'system'
})

afterEach(() => {
  i18n.global.locale.value = 'en'
  theme.value = 'system'
})

describe('expanded', () => {
  it('offers language and theme as two named selects and no gear', async () => {
    const { container } = mount(false)

    expect(screen.getByRole('combobox', { name: 'Language' })).toBeTruthy()
    expect(screen.getByRole('combobox', { name: 'Theme' })).toBeTruthy()
    expect(screen.queryByRole('button')).toBeNull()
    expect(await violations(container)).toBe('')
  })
})

describe('collapsed', () => {
  it('shows a gear named for both choices, not the theme icon named for one', async () => {
    const { container } = mount(true)

    const gear = screen.getByRole('button', { name: 'Language and theme' })
    expect(gear.getAttribute('icon')).toBe('i-lucide-settings-2')
    expect(gear.getAttribute('title')).toBe('Language and theme')
    expect(screen.queryByRole('combobox')).toBeNull()
    expect(await violations(container)).toBe('')
  })

  it('names the gear in all four languages', () => {
    const names = new Set<string>()
    for (const locale of SUPPORTED_LOCALES) {
      i18n.global.locale.value = locale
      const name = i18n.global.t('common.preferences.language_and_theme')
      expect(name).not.toBe('common.preferences.language_and_theme')
      names.add(name)
    }
    expect(names.size).toBe(SUPPORTED_LOCALES.length)
  })

  it('keeps every language and every theme selectable behind the gear', async () => {
    mount(true)

    const groups = screen.getAllByTestId('menu-group')
    expect(groups).toHaveLength(2)
    const labels = (group: HTMLElement) => Array.from(group.querySelectorAll('button')).map(button => button.textContent?.trim())
    expect(labels(groups[0]!)).toEqual(SUPPORTED_LOCALES.map(code => i18n.global.t(`common.locales.${code}`)))
    expect(labels(groups[1]!)).toEqual(['System', 'Light', 'Dark'])

    // The current choice is the checked one: English and the system theme to begin with.
    expect(screen.getByRole('button', { name: 'English' }).dataset.checked).toBe('true')
    expect(screen.getByRole('button', { name: 'System' }).dataset.checked).toBe('true')

    await fireEvent.click(screen.getByRole('button', { name: 'Deutsch' }))
    expect(i18n.global.locale.value).toBe('de')
    expect(screen.getByRole('button', { name: 'Deutsch' }).dataset.checked).toBe('true')
    expect(screen.getByRole('button', { name: 'English' }).dataset.checked).toBe('false')

    await fireEvent.click(screen.getByRole('button', { name: 'Dunkel' }))
    expect(theme.value).toBe('dark')
    expect(screen.getByRole('button', { name: 'Dunkel' }).dataset.checked).toBe('true')
  })
})
