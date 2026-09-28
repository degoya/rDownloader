/**
 * The action row, checked on the order and the shape the standard fixes (RD-150-11).
 *
 * The forms had nine spellings of "Cancel" and two rows that put the primary action last; the
 * row is one component now, so the order and the icon-only cross are asserted once, here, and
 * a form that renders `FormActions` inherits them.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import common from '@/locales/en/common.json'
import { mountComponent } from '@/test/mount'

import FormActions from './FormActions.vue'

function buttons(): HTMLButtonElement[] {
  return Array.from(document.querySelectorAll<HTMLButtonElement>('[data-form-actions] button'))
}

describe('FormActions', () => {
  it('offers only the create action while nothing is being edited', () => {
    mountComponent(FormActions, { props: { createLabel: 'Create category' } })
    const [submit, ...rest] = buttons()
    expect(submit?.textContent).toBe('Create category')
    expect(submit?.getAttribute('type')).toBe('submit')
    expect(submit?.getAttribute('icon')).toBe('i-lucide-plus')
    expect(rest).toHaveLength(0)
  })

  it('puts save first and the icon-only cancel second while editing', async () => {
    const view = mountComponent(FormActions, { props: { createLabel: 'Create category', editing: true } })
    const [submit, cancel] = buttons()
    expect(submit?.textContent).toBe(common.actions.save)
    expect(submit?.getAttribute('icon')).toBe('i-lucide-save')
    // The cross carries no visible text; its name and tooltip say what it does.
    expect(cancel?.textContent).toBe('')
    expect(cancel?.getAttribute('aria-label')).toBe(common.actions.cancel_edit)
    expect(cancel?.getAttribute('title')).toBe(common.actions.cancel_edit)
    expect(cancel?.getAttribute('icon')).toBe('i-lucide-x')
    expect(cancel?.getAttribute('type')).toBe('button')

    await fireEvent.click(screen.getByRole('button', { name: common.actions.cancel_edit }))
    expect(view.emitted('cancel')).toHaveLength(1)
  })

  it('offers the cross for a draft that is not an edit, and still creates', () => {
    mountComponent(FormActions, { props: { createLabel: 'Create hot folder', cancellable: true } })
    const [submit, cancel] = buttons()
    expect(submit?.textContent).toBe('Create hot folder')
    expect(cancel?.getAttribute('aria-label')).toBe(common.actions.cancel_edit)
  })

  it('places further actions after the two the standard fixes', () => {
    mountComponent({
      components: { FormActions },
      template: '<FormActions create-label="Create target" save-label="Save target" editing><button type="button">Test</button></FormActions>'
    })
    expect(buttons().map(button => button.textContent || button.getAttribute('aria-label')))
      .toEqual(['Save target', common.actions.cancel_edit, 'Test'])
  })

  it('submits the form it sits in when Enter is pressed in a field', async () => {
    let submitted = 0
    mountComponent({
      components: { FormActions },
      setup: () => ({ onSubmit: () => { submitted += 1 } }),
      template: '<form @submit.prevent="onSubmit"><input aria-label="Name" /><FormActions create-label="Create" /></form>'
    })
    // Enter submits a form through its submit button; pressing that button is the same path.
    await fireEvent.click(screen.getByRole('button', { name: 'Create' }))
    expect(submitted).toBe(1)
  })
})
