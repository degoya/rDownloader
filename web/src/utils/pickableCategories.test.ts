import { describe, expect, it } from 'vitest'

import type { Category } from '@/api/types'

import { pickableCategories } from './pickableCategories'

function category(id: string, selectable?: boolean): Category {
  return { id, name: id, ...(selectable === undefined ? {} : { selectable }) } as Category
}

/** The hand pickers' list (RD-1260-03). */
describe('pickableCategories', () => {
  const list = [category('films'), category('automation', false), category('series', true)]

  it('leaves out a category hidden from the selection, and keeps one that predates the field', () => {
    expect(pickableCategories(list).map(item => item.id)).toEqual(['films', 'series'])
  })

  it('keeps the hidden category an entry already lies in, in its place', () => {
    expect(pickableCategories(list, 'automation').map(item => item.id)).toEqual(['films', 'automation', 'series'])
  })

  it('keeps nothing extra for an entry without a category', () => {
    expect(pickableCategories(list, null).map(item => item.id)).toEqual(['films', 'series'])
  })
})
