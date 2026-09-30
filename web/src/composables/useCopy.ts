import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

/**
 * Puts `text` on the clipboard and says whether that worked.
 *
 * `navigator.clipboard` exists only in a secure context: opened over plain HTTP from another
 * machine on the LAN it is `undefined`, and a bare `writeText` threw where nobody caught it — a
 * token, an MFA recovery code or a command was silently not copied. The selection route through
 * a hidden text area still works there in the browsers that keep `execCommand('copy')`.
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch {
    // Refused (no permission, document not focused): the selection route below may still work.
  }
  return copyThroughSelection(text)
}

function copyThroughSelection(text: string): boolean {
  if (typeof document.execCommand !== 'function') return false
  const area = document.createElement('textarea')
  area.value = text
  area.setAttribute('readonly', '')
  area.style.position = 'fixed'
  area.style.opacity = '0'
  area.style.pointerEvents = 'none'
  document.body.appendChild(area)
  try {
    area.select()
    return document.execCommand('copy')
  } catch {
    return false
  } finally {
    area.remove()
  }
}

/**
 * `copyText` for a button: a copy that did not happen raises an error toast asking to copy by
 * hand, so the caller only announces success — when the promise resolves `true`.
 */
export function useCopy(): (text: string) => Promise<boolean> {
  const toast = useToast()
  const { t } = useI18n()
  return async (text: string): Promise<boolean> => {
    const copied = await copyText(text)
    if (!copied) {
      toast.add({
        title: t('common.copy.failed_title'),
        description: t('common.copy.failed_description'),
        color: 'error',
        icon: 'i-lucide-clipboard-x'
      })
    }
    return copied
  }
}
