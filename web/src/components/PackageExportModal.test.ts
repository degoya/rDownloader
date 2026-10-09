/**
 * The export dialog (RD-1210-01): the format, and a passphrase that is typed twice before it may
 * seal an `.rdlinks` file; a crawljob never carries one.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import common from '@/locales/en/common.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import PackageExportModal from './PackageExportModal.vue'

const en = common.export
const stubs = {
  UModal: { template: '<div><slot name="body" /><slot name="footer" /></div>' },
  // The shared stub, plus the error line the real field shows under its control.
  UFormField: {
    props: ['label', 'error'],
    template: '<div><label>{{ label }}<slot /></label><p v-if="error">{{ error }}</p></div>'
  }
}

function mountModal() {
  return mountComponent(PackageExportModal, { messages: { common }, stubs })
}

function submitButton(): HTMLButtonElement {
  return screen.getByTestId('package-export-submit') as HTMLButtonElement
}

async function submit(): Promise<void> {
  await fireEvent.submit(document.querySelector('#package-export-form') as HTMLFormElement)
}

async function type(testId: string, value: string): Promise<void> {
  await fireEvent.update(screen.getByTestId(testId), value)
}

describe('PackageExportModal', () => {
  it('writes a readable rdlinks file when no passphrase is given', async () => {
    const view = mountModal()
    expect(screen.getByText(en.format_rdlinks)).toBeTruthy()
    expect(screen.queryByTestId('package-export-confirmation')).toBeNull()
    await submit()
    expect(view.emitted('close')?.[0]).toEqual([{ format: 'rdlinks', passphrase: '' }])
  })

  it('seals only with a passphrase of eight characters typed twice alike', async () => {
    const view = mountModal()
    await type('package-export-passphrase', 'short')
    expect(screen.getByText(en.passphrase_short)).toBeTruthy()
    expect(submitButton().disabled).toBe(true)

    await type('package-export-passphrase', 'correct horse')
    await type('package-export-confirmation', 'correct hors')
    expect(screen.getByText(en.passphrase_mismatch)).toBeTruthy()
    await submit()
    expect(view.emitted('close')).toBeUndefined()

    await type('package-export-confirmation', 'correct horse')
    expect(submitButton().disabled).toBe(false)
    await submit()
    expect(view.emitted('close')?.[0]).toEqual([{ format: 'rdlinks', passphrase: 'correct horse' }])
  })

  it('drops the passphrase for a crawljob, which cannot be sealed', async () => {
    const view = mountModal()
    await type('package-export-passphrase', 'correct horse')
    const crawljob = document.querySelector('input[type="radio"][value="crawljob"]') as HTMLInputElement
    await fireEvent.change(crawljob)
    expect(screen.queryByTestId('package-export-passphrase')).toBeNull()
    await submit()
    expect(view.emitted('close')?.[0]).toEqual([{ format: 'crawljob', passphrase: '' }])
  })

  it('has no accessibility violations', async () => {
    const view = mountModal()
    await type('package-export-passphrase', 'correct horse')
    expect(await axeViolations(view.container)).toBe('')
  })
})
