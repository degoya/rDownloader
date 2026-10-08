/**
 * The pick list of things you create yourself (RD-1180-02): a plain select while the list is
 * short, Nuxt UI's select menu with its search field from eight entries on.
 *
 * The long list is mounted against the real `SelectMenu.vue` over Reka's combobox, not a stub —
 * what the owner asked for is the search's own behaviour (a part of the name, any case, typing
 * at once, Enter picks), and a stub would only test itself. Its Nuxt build modules come from
 * `src/test/nuxtUi/` (`vitest.config.ts`); icons, avatars, chips and buttons are blanks here.
 */
import SelectMenu from '@nuxt/ui/components/SelectMenu.vue'
import { fireEvent, waitFor, within } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import SearchableSelect from '@/components/SearchableSelect.vue'
import deCommon from '@/locales/de/common.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

vi.mock('@nuxt/ui/components/Icon.vue', () => ({ default: { template: '<span />' } }))
vi.mock('@nuxt/ui/components/Avatar.vue', () => ({ default: { template: '<span />' } }))
vi.mock('@nuxt/ui/components/Chip.vue', () => ({ default: { template: '<span />' } }))
vi.mock('@nuxt/ui/components/Button.vue', () => ({ default: { template: '<button type="button" />' } }))

// Reka's popper measures its content and scrolls the highlighted option into view; jsdom has no
// layout to observe or scroll.
globalThis.ResizeObserver ??= class {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
} as unknown as typeof ResizeObserver
Element.prototype.scrollIntoView ??= () => {}

const DISKS = ['Downloads HDD-1', 'Downloads HDD-2', 'Filme HDD-1', 'Filme HDD-3', 'Musik HDD-2', 'Serien HDD-1', 'Serien HDD-3', 'Serien HDD-4']
  .map((name, index) => ({ label: name, value: `cat-${index}` }))

function renderSelect(items = DISKS, extra: Record<string, unknown> = {}, locale?: string) {
  return mountComponent(SearchableSelect, {
    props: { items, modelValue: 'cat-1', 'aria-label': 'Category', ...extra },
    stubs: { USelectMenu: SelectMenu },
    ...(locale ? { locale, messages: { common: deCommon } } : {})
  })
}

/** The options of the open menu, by their visible label. */
function options(): string[] {
  return within(document.body).queryAllByRole('option').map(option => option.textContent?.trim() ?? '')
}

describe('SearchableSelect', () => {
  it('stays a plain select below eight entries', () => {
    const view = renderSelect(DISKS.slice(0, 7))
    const field = view.getByRole('combobox', { name: 'Category' })
    expect(field.tagName).toBe('SELECT')
    expect((field as HTMLSelectElement).value).toBe('cat-1')
  })

  it('hands the plain select its value and every attribute', async () => {
    const view = renderSelect(DISKS.slice(0, 3), { 'data-testid': 'category', class: 'w-36' })
    const field = view.getByTestId('category')
    expect(field.classList.contains('w-36')).toBe(true)
    await fireEvent.update(field, 'cat-2')
    expect(view.emitted('update:modelValue')).toEqual([['cat-2']])
  })

  it('opens a search field from eight entries on', async () => {
    const view = renderSelect()
    const trigger = view.getByRole('button', { name: 'Category' })
    expect(trigger.tagName).toBe('BUTTON')
    expect(trigger.textContent).toContain('Downloads HDD-2')
    await fireEvent.click(trigger)
    const search = await within(document.body).findByPlaceholderText('Search…')
    expect(options()).toHaveLength(8)
    expect(search).toBeTruthy()
  })

  it('finds a part of the name, in any case', async () => {
    const view = renderSelect()
    await fireEvent.click(view.getByRole('button', { name: 'Category' }))
    const search = await within(document.body).findByPlaceholderText('Search…')
    await fireEvent.update(search, 'hdd-4')
    await waitFor(() => expect(options()).toEqual(['Serien HDD-4']))
    await fireEvent.update(search, 'FILME')
    await waitFor(() => expect(options()).toEqual(['Filme HDD-1', 'Filme HDD-3']))
  })

  it('says so when nothing matches', async () => {
    const view = renderSelect()
    await fireEvent.click(view.getByRole('button', { name: 'Category' }))
    await fireEvent.update(await within(document.body).findByPlaceholderText('Search…'), 'ssd')
    await waitFor(() => expect(options()).toEqual([]))
    expect(within(document.body).getByText('Nothing found')).toBeTruthy()
  })

  it('searches for a letter typed on the closed field', async () => {
    const view = renderSelect()
    const trigger = view.getByRole('button', { name: 'Category' })
    await fireEvent.keyDown(trigger, { key: 'm' })
    const search = await within(document.body).findByPlaceholderText('Search…') as HTMLInputElement
    expect(search.value).toBe('m')
    await waitFor(() => expect(options()).toEqual(['Musik HDD-2', 'Filme HDD-1', 'Filme HDD-3']))
  })

  it('leaves a shortcut, the space bar and the arrows to the field', async () => {
    const view = renderSelect()
    const trigger = view.getByRole('button', { name: 'Category' })
    await fireEvent.keyDown(trigger, { key: 'k', ctrlKey: true })
    await fireEvent.keyDown(trigger, { key: ' ' })
    expect(within(document.body).queryByPlaceholderText('Search…')).toBeNull()
  })

  it('picks the first match with Enter', async () => {
    const view = renderSelect()
    await fireEvent.click(view.getByRole('button', { name: 'Category' }))
    const search = await within(document.body).findByPlaceholderText('Search…')
    await fireEvent.update(search, 'serien hdd')
    await waitFor(() => expect(options()).toEqual(['Serien HDD-1', 'Serien HDD-3', 'Serien HDD-4']))
    await waitFor(() => expect(within(document.body).getByRole('option', { name: 'Serien HDD-1' }).hasAttribute('data-highlighted')).toBe(true))
    await fireEvent.keyDown(search, { key: 'Enter' })
    await waitFor(() => expect(view.emitted('update:modelValue')).toEqual([['cat-5']]))
  })

  it('speaks the language of the interface', async () => {
    const view = renderSelect(DISKS, {}, 'de')
    await fireEvent.click(view.getByRole('button', { name: 'Category' }))
    const search = await within(document.body).findByPlaceholderText('Suchen …')
    await fireEvent.update(search, 'ssd')
    await waitFor(() => expect(within(document.body).getByText('Nichts gefunden')).toBeTruthy())
  })

  it('has no accessibility violations, closed or open', async () => {
    const view = renderSelect()
    expect(await axeViolations(view.container)).toBe('')
    await fireEvent.click(view.getByRole('button', { name: 'Category' }))
    await within(document.body).findByPlaceholderText('Search…')
    expect(await axeViolations(document.body.lastElementChild as Element)).toBe('')
  })
})
