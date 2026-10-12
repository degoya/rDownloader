/**
 * The pickers that assign a category by hand leave out one hidden from the selection, and show
 * the one an entry already lies in (RD-1260-03). The selection bar stands for both lists; the
 * LinkGrabber's package row for the rows of both.
 */
import { describe, expect, it } from 'vitest'

import type { Category, CollectorPackage } from '@/api/types'
import downloads from '@/locales/en/downloads.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import { mountComponent } from '@/test/mount'

import BulkActionBar from './BulkActionBar.vue'
import CollectorPackageGroup from './CollectorPackageGroup.vue'

/** Draws the items as options, so a test reads what the picker offers. */
const SearchableSelect = {
  props: ['modelValue', 'items'],
  template: '<select v-bind="$attrs"><option v-for="item in items" :key="item.value" :value="item.value">{{ item.label }}</option></select>'
}

const CATEGORIES = [
  { id: 'films', name: 'Films', selectable: true },
  { id: 'automation', name: 'Automation', selectable: false },
  { id: 'series', name: 'Series' }
] as Category[]

function offered(select: Element | null): string[] {
  return [...(select?.querySelectorAll('option') ?? [])].map(option => option.textContent ?? '')
}

describe('category pickers', () => {
  it('offers the selection bar only the categories that are not hidden', () => {
    const { getByLabelText } = mountComponent(BulkActionBar, {
      messages: { downloads },
      props: { count: 2, categories: CATEGORIES, transferActions: true },
      stubs: { SearchableSelect }
    })
    expect(offered(getByLabelText(downloads.bulk.category_aria))).toEqual([downloads.bulk.default_category, 'Films', 'Series'])
  })

  function packageRow(categoryId: string | null) {
    return mountComponent(CollectorPackageGroup, {
      messages: { linkgrabber },
      props: {
        package: { id: 'package-1', name: 'Report', priority: 'normal', has_password: false, category_id: categoryId } as CollectorPackage,
        candidates: [],
        categories: CATEGORIES,
        selectedIds: new Set<string>(),
        enqueuingIds: new Set<string>(),
        dragging: false,
        open: false
      },
      stubs: { SearchableSelect }
    })
  }

  it('leaves a hidden category out of a package row that is not in it', () => {
    const { getByLabelText } = packageRow('films')
    expect(offered(getByLabelText(linkgrabber.package.category))).toEqual([linkgrabber.package.default_category, 'Films', 'Series'])
  })

  it('shows the hidden category a package already lies in as its value', () => {
    const { getByLabelText } = packageRow('automation')
    expect(offered(getByLabelText(linkgrabber.package.category))).toEqual([linkgrabber.package.default_category, 'Films', 'Automation', 'Series'])
  })
})
