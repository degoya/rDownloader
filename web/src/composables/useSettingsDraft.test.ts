/**
 * RD-1480-07: the settings page sends what changed, and a change made elsewhere — by event or
 * found at the save — neither loses unsaved input nor is overwritten unseen.
 */
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock, eventStreamMock } from '@/test/mocks'
import { ref } from 'vue'

import { api } from '@/api/client'
import { defaultSettings } from '@/settingsDefaults'
import { mountComponent, passTime } from '@/test/mount'

const handlers = vi.hoisted(() => new Map<string, (event: MessageEvent) => void>())
vi.mock('@/api/client', () => apiClientMock())
vi.mock('@/composables/useEventStream', () => eventStreamMock({
  subscribeEvents: (map: Record<string, (event: MessageEvent) => void>) => {
    for (const [name, handler] of Object.entries(map)) handlers.set(name, handler)
    return () => handlers.clear()
  }
}))

import { useSettingsDraft } from './useSettingsDraft'

type Document = ReturnType<typeof defaultSettings>
type Draft = ReturnType<typeof useSettingsDraft>

/** The service: one document, a tag per version, `If-Match` honoured. */
let stored: Document
/** Across the tests: the page remembers the tags of its own writes for as long as it is open. */
let version = 0
let first: string
const sent: { body: object, ifMatch: string | undefined }[] = []

function answer(data: object | undefined, status = 200, error?: object) {
  return { data, error, response: new Response(null, { status, headers: { ETag: `"v${version}"` } }) }
}

function announce(): void {
  handlers.get('settings.changed')?.(new MessageEvent('settings.changed'))
}

/** Another writer changes the document, with or without its event reaching this page. */
async function changedElsewhere(fields: Partial<Document>, announced = true): Promise<void> {
  stored = { ...stored, ...fields }
  version += 1
  if (!announced) return
  await passTime(300, announce)
}

/** A page whose form shows the stored document. */
async function draft(): Promise<Draft & { onStored: ReturnType<typeof vi.fn> }> {
  const loaded = ref(false)
  const onStored = vi.fn()
  let made: Draft | undefined
  mountComponent({
    setup() {
      made = useSettingsDraft({ loaded: () => loaded.value, busy: () => false, onStored })
      return () => null
    }
  }, { keepPinia: true })
  const page = made as unknown as Draft
  page.replace(answer(structuredClone(stored)) as never)
  loaded.value = true
  return { ...page, onStored }
}

describe('the settings draft', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    handlers.clear()
    sent.length = 0
    stored = { ...defaultSettings(), max_retries: 4, media_hosts: ['a.example'] }
    version += 1
    first = `"v${version}"`
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.GET).mockImplementation((async () => answer(structuredClone(stored))) as never)
    vi.mocked(api.PATCH).mockReset()
    vi.mocked(api.PATCH).mockImplementation((async (_path: string, init: { body: object, params?: { header: { 'If-Match': string } } }) => {
      const ifMatch = init.params?.header['If-Match']
      sent.push({ body: init.body, ifMatch })
      if (ifMatch && ifMatch !== `"v${version}"`) return answer(undefined, 412, { code: 'settings.document_changed' })
      stored = { ...stored, ...init.body }
      version += 1
      return answer(structuredClone(stored))
    }) as never)
  })

  it('sends the changed fields with the tag they were read with, and nothing when nothing changed', async () => {
    const page = await draft()
    expect(page.dirty()).toBe(false)
    expect(await page.save()).toEqual({ saved: true })
    expect(sent).toEqual([])

    page.settings.max_retries = 9
    page.settings.media_hosts.push('https://www.B.example/videos')
    page.speedMiB.value = 2
    // A card that reads its field through a cast (RD-1480-11).
    Object.assign(page.settings, { notification_language: 'de' })
    expect(page.dirty()).toBe(true)
    expect(await page.save()).toEqual({ saved: true })
    expect(sent).toEqual([{
      body: { max_retries: 9, media_hosts: ['a.example', 'b.example'], speed_limit_bytes_per_second: '2097152', notification_language: 'de' },
      ifMatch: first
    }])
    expect(page.settings.media_hosts).toEqual(['a.example', 'b.example'])
    expect(page.dirty()).toBe(false)
    expect(page.foreign.value).toBeNull()
  })

  it('shows a change made elsewhere and keeps the unsaved input', async () => {
    const page = await draft()
    page.settings.max_retries = 9
    await changedElsewhere({ max_active_files: 12, speed_limit_bytes_per_second: '1048576' })

    expect(page.settings.max_active_files).toBe(12)
    expect(page.speedMiB.value).toBe(1)
    expect(page.settings.max_retries).toBe(9)
    expect(page.foreign.value).toEqual({ conflicts: [] })
    expect(page.onStored).toHaveBeenLastCalledWith(expect.objectContaining({ max_active_files: 12 }))

    expect(await page.save()).toEqual({ saved: true })
    expect(sent).toEqual([{ body: { max_retries: 9 }, ifMatch: `"v${version - 1}"` }])
    expect(stored.max_active_files).toBe(12)
  })

  it('names a field both sides changed, and either value can win', async () => {
    const page = await draft()
    page.settings.max_retries = 9
    await changedElsewhere({ max_retries: 5 })
    expect(page.settings.max_retries).toBe(9)
    expect(page.foreign.value).toEqual({ conflicts: ['max_retries'] })

    page.takeForeign()
    expect(page.settings.max_retries).toBe(5)
    expect(page.dirty()).toBe(false)
    expect(page.foreign.value).toBeNull()

    page.settings.max_retries = 8
    await changedElsewhere({ max_retries: 6 })
    expect(await page.save()).toEqual({ saved: true })
    expect(stored.max_retries).toBe(8)
    expect(page.foreign.value).toBeNull()
  })

  it('finds at the save what no event announced: it goes on beside it, and stops at the same field', async () => {
    const page = await draft()
    page.settings.max_retries = 9
    await changedElsewhere({ max_active_files: 12 }, false)
    expect(await page.save()).toEqual({ saved: true })
    expect(sent.map(request => request.ifMatch)).toEqual([first, `"v${version - 1}"`])
    expect([stored.max_retries, stored.max_active_files]).toEqual([9, 12])
    expect(page.foreign.value).toEqual({ conflicts: [] })

    page.settings.max_retries = 7
    await changedElsewhere({ max_retries: 1 }, false)
    expect(await page.save()).toEqual({ conflict: true })
    expect(stored.max_retries).toBe(1)
    expect(page.settings.max_retries).toBe(7)
    expect(page.foreign.value).toEqual({ conflicts: ['max_retries'] })

    expect(await page.save()).toEqual({ saved: true })
    expect(stored.max_retries).toBe(7)
  })

  it('takes the event of its own save for no one else’s change', async () => {
    const page = await draft()
    page.settings.max_retries = 9
    await page.save()
    await passTime(300, announce)
    expect(api.GET).toHaveBeenCalledTimes(1)
    expect(page.foreign.value).toBeNull()
  })

  it('keeps what was typed while the save was on its way', async () => {
    const page = await draft()
    page.settings.max_retries = 9
    const saving = page.save()
    page.settings.max_retries = 3
    expect(await saving).toEqual({ saved: true })
    expect(page.settings.max_retries).toBe(3)
    expect(page.dirty()).toBe(true)
  })

  it('answers the refusal and keeps the input', async () => {
    const page = await draft()
    vi.mocked(api.PATCH).mockImplementation((async () => answer(undefined, 400, { code: 'settings.max_retries_too_high' })) as never)
    page.settings.max_retries = 500
    const outcome = await page.save()
    expect(outcome).toMatchObject({ refused: { error: { code: 'settings.max_retries_too_high' } } })
    expect(page.settings.max_retries).toBe(500)
    expect(page.dirty()).toBe(true)
  })
})
