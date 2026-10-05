/**
 * RD-1100-08: the category's sort templates and their preview. The preview asks the service —
 * the same recognition and expansion the sort runs — so what the form shows before saving is
 * what a finished file gets.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import routing from '@/locales/en/routing.json'
import server from '@/locales/en/server.json'
import { sortingBody, sortingForm } from '@/composables/useCategoryForm'
import { mountComponent } from '@/test/mount'

const post = vi.fn()
vi.mock('@/api/client', () => ({
  api: { POST: (...args: unknown[]) => post(...args) }
}))

const { default: RoutingCategorySorting } = await import('./RoutingCategorySorting.vue')

const SERIES = '{show}/Season {season:00}/{show} - S{season:00}E{episode:00} - {title}'
const FIELDS = { series: ['show', 'season', 'episode', 'title'], dated: ['show', 'date'], movie: ['movie', 'year'] }

function mount(series = SERIES) {
  return mountComponent(RoutingCategorySorting, {
    messages: { routing, server },
    props: { modelValue: { series, dated: '', movie: '' } }
  })
}

describe('RoutingCategorySorting', () => {
  beforeEach(() => {
    post.mockReset()
  })

  it('shows where each example name would land before anything is saved', async () => {
    post.mockResolvedValue({
      data: {
        entries: [
          { name: 'Show.Name.S01E03E04.1080p.WEB-DL.mkv', kind: 'series', fields: {}, path: 'Show Name/Season 01/Show Name - S01E03-E04.mkv', code: null },
          { name: 'Film.Name.2010.1080p.BluRay.x264-GRP.mkv', kind: 'movie', fields: {}, path: null, code: 'sort.no_template' },
          { name: 'holiday.mp4', kind: null, fields: {}, path: null, code: null }
        ],
        fields: FIELDS
      }
    })

    mount()

    await waitFor(() => expect(screen.getByText('Show Name/Season 01/Show Name - S01E03-E04.mkv')).toBeTruthy())
    const [path, request] = post.mock.calls[0] as [string, { body: { sorting: Record<string, string | null>, names: string[] } }]
    expect(path).toBe('/api/v1/postprocess/sort-preview')
    expect(request.body.sorting).toEqual({ series: SERIES, dated: null, movie: null })
    expect(request.body.names.length).toBeGreaterThan(0)
    expect(screen.getByText(server.codes['sort.no_template'])).toBeTruthy()
    expect(screen.getByText(routing.category.sorting_unrecognised)).toBeTruthy()
    expect(screen.getByText(routing.category.sorting_kind_series)).toBeTruthy()
  })

  it('names the template the service refused', async () => {
    post.mockResolvedValue({
      error: { code: 'sort.template_outside', message: 'series template: outside', params: { kind: 'series' } }
    })

    mount('../{show}/{title}')

    await waitFor(() => expect(post).toHaveBeenCalled())
    const field = (screen.getByTestId('sorting-series') as HTMLElement).closest('div') as HTMLElement
    await waitFor(() => expect(field.getAttribute('error')).toBe(server.codes['sort.template_outside']))
    expect(screen.queryByTestId('sorting-preview')).toBeNull()
  })

  it('asks nothing while every template is empty, and again once one is typed', async () => {
    post.mockResolvedValue({ data: { entries: [], fields: FIELDS } })

    mount('')
    await new Promise(resolve => setTimeout(resolve, 400))
    expect(post).not.toHaveBeenCalled()

    await fireEvent.update(screen.getByTestId('sorting-movie'), '{movie} ({year})')
    await waitFor(() => expect(post).toHaveBeenCalledTimes(1), { timeout: 2000 })
    expect((post.mock.calls[0]?.[1] as { body: { sorting: unknown } }).body.sorting)
      .toEqual({ series: null, dated: null, movie: '{movie} ({year})' })
  })
})

describe('sort templates in the category body', () => {
  it('drops blank templates and sends no sorting when none is left', () => {
    expect(sortingBody({ series: '  ', dated: '', movie: '' })).toBeNull()
    expect(sortingBody({ series: ' {show}/{show} {episode} ', dated: '', movie: '' }))
      .toEqual({ series: '{show}/{show} {episode}', dated: null, movie: null })
    expect(sortingForm(null)).toEqual({ series: '', dated: '', movie: '' })
    expect(sortingForm({ movie: '{movie}' })).toEqual({ series: '', dated: '', movie: '{movie}' })
  })
})
