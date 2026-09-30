import { useEventListener } from '@vueuse/core'
import { type MaybeRefOrGetter, toValue } from 'vue'
import { useI18n } from 'vue-i18n'
import { onBeforeRouteLeave, onBeforeRouteUpdate, type RouteLocationNormalized } from 'vue-router'

import { useConfirm } from '@/composables/useConfirm'

export interface UnsavedGuardOptions {
  /**
   * Whether a navigation that keeps the view mounted — another param or query of the same route —
   * still drops edits, because the part holding them unmounts. Unset, such a navigation keeps the
   * view and its edits and is never asked about.
   */
  dropsEdits?: (to: RouteLocationNormalized, from: RouteLocationNormalized) => boolean
}

/**
 * Asks before unsaved edits are lost (RD-180-16): leaving the view through the router asks in
 * the app's own confirmation — *Discard* leaves, *Cancel* stays — and closing or reloading the
 * tab gets the browser's question, the only one a page may raise there. A clean view asks
 * nothing.
 *
 * Call it in the setup of the view the route renders: the router's in-component guards only
 * reach a component inside `RouterView`.
 */
export function useUnsavedGuard(dirty: MaybeRefOrGetter<boolean>, options: UnsavedGuardOptions = {}): void {
  const { t } = useI18n()
  const confirm = useConfirm()

  const mayDiscard = (): Promise<boolean> => confirm({
    title: t('common.unsaved.title'),
    description: t('common.unsaved.description'),
    confirmLabel: t('common.unsaved.discard'),
    confirmIcon: 'i-lucide-undo-2',
    destructive: true
  })

  onBeforeRouteLeave(async () => !toValue(dirty) || await mayDiscard())
  const { dropsEdits } = options
  if (dropsEdits) onBeforeRouteUpdate(async (to, from) => !dropsEdits(to, from) || await mayDiscard())
  useEventListener(window, 'beforeunload', (event: BeforeUnloadEvent) => {
    if (toValue(dirty)) event.preventDefault()
  })
}
