import { computed, ref, type ComputedRef } from 'vue'

interface FormBaseline {
  /** True once the form differs from what it was last opened with. */
  dirty: ComputedRef<boolean>
  /** Takes the form as it is now as the state to compare against: after a reset, a fill, a save. */
  settle: () => void
}

/**
 * Whether a form outside the settings document holds edits that are not saved (RD-1120-15), for
 * `useUnsavedGuard`. The form is compared as JSON, the way the settings page compares its
 * document, so a value typed and typed back again is not an edit.
 */
export function useFormBaseline(current: () => unknown): FormBaseline {
  const snapshot = (): string => JSON.stringify(current())
  const baseline = ref(snapshot())
  return {
    dirty: computed(() => snapshot() !== baseline.value),
    settle: () => { baseline.value = snapshot() }
  }
}
