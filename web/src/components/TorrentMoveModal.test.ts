/**
 * "Change location" of a torrent (RD-1100-10): the dialog sends a storage root and a folder below
 * it, keeps a refusal in view, and closes with the server's message once the move has started.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import common from '@/locales/en/common.json'
import torrent from '@/locales/en/torrent.json'
import { mountComponent } from '@/test/mount'

import TorrentMoveModal from './TorrentMoveModal.vue'

const calls = vi.hoisted(() => ({ post: [] as unknown[], answer: null as unknown }))

vi.mock('@/api/client', () => ({
  api: {
    GET: vi.fn(async () => ({
      data: [
        { id: 'r1', name: 'Media', path: '/srv/media', is_default: false },
        { id: 'r2', name: 'Archive', path: '/srv/archive', is_default: true }
      ]
    })),
    POST: vi.fn(async (_path: string, options: unknown) => {
      calls.post.push(options)
      return calls.answer
    })
  },
  responseError: (response: { error?: { message?: string } }) => response.error?.message ?? 'failed',
  resultMessage: (body: { message?: string }) => body.message ?? ''
}))

/** The dialog's body is a named slot; the shared passthrough stub renders only the default one. */
const UModal = { template: '<div><slot name="body" /><slot name="footer" /></div>' }
const UFormField = { props: ['label'], template: '<label>{{ label }}<slot /></label>' }
const USelect = {
  props: ['modelValue', 'items'],
  template: '<select :value="modelValue"><option v-for="item in items" :key="item.value" :value="item.value">{{ item.label }}</option></select>'
}

function mount() {
  return mountComponent(TorrentMoveModal, {
    props: { downloadId: 'd1' },
    messages: { common, torrent },
    stubs: { UModal, UFormField, USelect }
  })
}

beforeEach(() => {
  calls.post = []
})

describe('TorrentMoveModal', () => {
  it('moves into the default root and the folder typed, and closes with the message', async () => {
    calls.answer = { data: { code: 'torrent.move_started', message: 'Moving the torrent\'s files' } }
    const view = mount()
    await flushPromises()
    await fireEvent.update(screen.getByPlaceholderText(torrent.move.path_placeholder), '  Series/2026 ')
    await fireEvent.submit(document.getElementById('torrent-move-form') as HTMLFormElement)
    await flushPromises()

    expect(calls.post).toEqual([{
      params: { path: { id: 'd1' } },
      body: { storage_root_id: 'r2', relative_path: 'Series/2026' }
    }])
    expect(view.emitted('close')).toEqual([['Moving the torrent\'s files']])
  })

  it('keeps a refusal in the dialog instead of closing', async () => {
    calls.answer = { error: { message: 'A folder of that name is already there' } }
    const view = mount()
    await flushPromises()
    await fireEvent.submit(document.getElementById('torrent-move-form') as HTMLFormElement)
    await flushPromises()

    expect(screen.getByText('A folder of that name is already there')).toBeTruthy()
    expect(view.emitted('close')).toBeUndefined()
  })
})
