/**
 * The category filter of the bundled services is a radio group, the chosen category checked and
 * each one's count beside its name (RD-1120-14).
 */
import { fireEvent, screen, within } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { BundledService } from '@/api/bundledPlugins'
import plugins from '@/locales/en/plugins.json'
import { mountComponent } from '@/test/mount'

import BundledServicePicker from './BundledServicePicker.vue'

const services = [
  { key: 'a', name: 'Alpha host', category: 'hoster', description: '', state: 'missing', needs_account: false },
  { key: 'b', name: 'Beta host', category: 'hoster', description: '', state: 'missing', needs_account: false },
  { key: 'c', name: 'Gamma metadata', category: 'metadata', description: '', state: 'missing', needs_account: false }
] as unknown as BundledService[]

describe('BundledServicePicker', () => {
  it('filters by the category whose radio is checked', async () => {
    mountComponent(BundledServicePicker, { messages: { plugins }, props: { services, mode: 'install' } })
    const filter = screen.getByRole('group', { name: plugins.bundled.filter_label })
    const all = within(filter).getByRole('radio', { name: `${plugins.bundled.all} 3` }) as HTMLInputElement
    expect(all.checked).toBe(true)
    const hoster = within(filter).getByRole('radio', { name: `${plugins.bundled.category.hoster} 2` }) as HTMLInputElement
    await fireEvent.click(hoster)
    expect(hoster.checked).toBe(true)
    expect(all.checked).toBe(false)
    expect(screen.queryByText('Gamma metadata')).toBeNull()
    expect(screen.getByText('Alpha host')).toBeTruthy()
  })
})
