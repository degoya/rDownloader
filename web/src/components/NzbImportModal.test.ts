/**
 * The import modal's file field (RD-1110-12): a `UFileUpload` whose `accept` names extensions
 * only, taking dropped and chosen files alike; what the import cannot read is left out, and a
 * file already listed is not listed twice.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import linkgrabber from '@/locales/en/linkgrabber.json'
import { mountComponent } from '@/test/mount'

import NzbImportModal from './NzbImportModal.vue'

const en = linkgrabber.nzb.modal
const modal = { UModal: { template: '<div><slot name="body" /><slot name="footer" /></div>' } }

function mountModal() {
  return mountComponent(NzbImportModal, {
    messages: { linkgrabber },
    props: { categories: [] },
    stubs: modal
  })
}

/** Drops `files` on the field's area, as a browser that names no MIME type for them would. */
async function drop(zone: HTMLElement, files: File[]): Promise<void> {
  const dataTransfer = { items: files.map(() => ({ kind: 'file', type: '' })), files, types: ['Files'] }
  await fireEvent.dragEnter(zone, { dataTransfer })
  await fireEvent.drop(zone, { dataTransfer })
}

describe('NzbImportModal file field', () => {
  it('takes dropped and chosen files, leaves out what it cannot import, and imports them', async () => {
    const view = mountModal()
    const input = document.querySelector('input[type="file"]') as HTMLInputElement
    expect(input.accept).toBe('.nzb,.torrent,.dlc,.ccf,.rsdf,.txt')
    expect(input.multiple).toBe(true)

    const release = new File(['<nzb/>'], 'Release {{secret}}.nzb')
    await drop(document.querySelector('[data-file-drop]') as HTMLElement, [release, new File(['x'], 'setup.exe')])
    await waitFor(() => expect(screen.getByText('Release {{secret}}.nzb')).toBeTruthy())
    expect(screen.queryByText('setup.exe')).toBeNull()
    expect(screen.getByText('secret')).toBeTruthy()

    const torrent = new File(['d4:infoe'], 'show.torrent')
    Object.defineProperty(input, 'files', { value: [torrent, release], configurable: true })
    await fireEvent.change(input)
    await waitFor(() => expect(screen.getByText('show.torrent')).toBeTruthy())
    expect(screen.getAllByText('Release {{secret}}.nzb')).toHaveLength(1)

    await fireEvent.submit(document.querySelector('#nzb-import-form') as HTMLFormElement)
    const [result] = view.emitted('close')?.[0] as [{ entries: { file: File, name: string }[] }]
    expect(result.entries.map(entry => entry.name)).toEqual(['Release{{secret}}', 'show'])
    expect(screen.getByText(en.choose_file)).toBeTruthy()
  })
})
