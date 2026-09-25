import { nextTick, type Ref } from 'vue'

/**
 * Moves the focus into the first field of a form (RD-106-15).
 *
 * Called from a row's edit action once the form holds the row's values. On a wide screen the
 * form stands beside the list and the cursor lands where the change is made; on a narrow one
 * the form is above the list, and the focus move is what scrolls it into view. Nuxt UI renders
 * a select as a button with `role="combobox"`, so that counts as a field too.
 */
export function useFormFocus(form: Ref<HTMLElement | null>) {
  return async function focusFirstField(): Promise<void> {
    await nextTick()
    form.value
      ?.querySelector<HTMLElement>('input:not([type="hidden"]), textarea, select, [role="combobox"]')
      ?.focus()
  }
}
