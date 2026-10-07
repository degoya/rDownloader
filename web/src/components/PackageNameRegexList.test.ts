/**
 * RD-1140-05: the list of package-name regex rules — added and edited through the regex editor's
 * replacement mode, removed and reordered here, at most ten.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { PackageNameRegex } from '@/api/types'
import settings from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

const editPair = vi.fn()
vi.mock('@/composables/usePackageNameRegexEditor', () => ({ usePackageNameRegexEditor: () => editPair }))

const { default: PackageNameRegexList } = await import('./PackageNameRegexList.vue')

const labels = settings.postprocess.package_names.regex
const pair = (pattern: string, replacement = ''): PackageNameRegex => ({ pattern, replacement })

function mount(pairs: PackageNameRegex[]) {
  const updates: PackageNameRegex[][] = []
  mountComponent(PackageNameRegexList, {
    messages: { settings },
    props: { 'modelValue': pairs, 'onUpdate:modelValue': (value: PackageNameRegex[]) => updates.push(value) }
  })
  return updates
}

describe('PackageNameRegexList', () => {
  beforeEach(() => editPair.mockReset())

  it('adds the pair the editor hands back at the end', async () => {
    editPair.mockResolvedValue(pair('_', '.'))
    const updates = mount([pair('a')])

    await fireEvent.click(screen.getByTestId('package-name-regex-add'))

    await vi.waitFor(() => expect(updates.at(-1)).toEqual([pair('a'), pair('_', '.')]))
    expect(editPair).toHaveBeenCalledWith(null)
  })

  it('edits, moves and removes a pair in place', async () => {
    editPair.mockResolvedValue(pair('b2', 'x'))
    const updates = mount([pair('a'), pair('b'), pair('c')])

    await fireEvent.click(screen.getAllByRole('button', { name: labels.edit })[1] as HTMLElement)
    await vi.waitFor(() => expect(updates.at(-1)).toEqual([pair('a'), pair('b2', 'x'), pair('c')]))
    await fireEvent.click(screen.getAllByRole('button', { name: labels.move_up })[2] as HTMLElement)
    expect(updates.at(-1)).toEqual([pair('a'), pair('c'), pair('b')])
    await fireEvent.click(screen.getAllByRole('button', { name: labels.remove })[0] as HTMLElement)
    expect(updates.at(-1)).toEqual([pair('b'), pair('c')])
  })

  it('offers no eleventh rule', () => {
    mount(Array.from({ length: 10 }, (_, index) => pair(String(index))))
    expect((screen.getByTestId('package-name-regex-add') as HTMLButtonElement).disabled).toBe(true)
  })
})
