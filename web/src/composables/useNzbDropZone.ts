import { useToast } from '@nuxt/ui/composables'
import { useEventListener } from '@vueuse/core'
import { ref, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'

import { fileDropClaim, filterImportFiles, isFileImportModalOpen, requestFileImport } from '@/composables/nzbImportRequest'

function dragCarriesFiles(event: DragEvent): boolean {
  return Array.from(event.dataTransfer?.types ?? []).includes('Files')
}

/**
 * App-wide metadata drag & drop: shows a full-screen overlay while files are dragged over the
 * window and, on drop, hands matching `.nzb`/`.torrent` files to the LinkGrabber view
 * state in `nzbImportRequest.ts`. No-ops while the import modal is already open (it has its
 * own drop zone), and hands the drop to a page that claimed it (`claimFileDrops`) instead.
 */
export function useFileImportDropZone(): { dropActive: Ref<boolean> } {
  const dropActive = ref(false)
  const router = useRouter()
  const toast = useToast()
  const { t } = useI18n()

  let depth = 0

  function reset(): void {
    depth = 0
    dropActive.value = false
  }

  useEventListener(window, 'dragenter', (event: DragEvent) => {
    if (isFileImportModalOpen() || fileDropClaim() || !dragCarriesFiles(event)) return
    depth += 1
    dropActive.value = true
  })

  useEventListener(window, 'dragover', (event: DragEvent) => {
    // Always prevented (even while the modal is open) so the browser never opens the file itself.
    if (!dragCarriesFiles(event)) return
    event.preventDefault()
  })

  useEventListener(window, 'dragleave', (event: DragEvent) => {
    if (isFileImportModalOpen() || !dragCarriesFiles(event)) return
    depth = Math.max(0, depth - 1)
    dropActive.value = depth > 0
  })

  useEventListener(window, 'drop', (event: DragEvent) => {
    if (!dragCarriesFiles(event)) return
    event.preventDefault()
    reset()
    if (isFileImportModalOpen()) return
    // A page that takes files itself gets all of them, unfiltered: it says per file what it
    // refused, which a toast about "no matching file" here would not.
    const claim = fileDropClaim()
    if (claim) return void claim(Array.from(event.dataTransfer?.files ?? []))
    const matched = filterImportFiles(event.dataTransfer?.files ?? [])
    if (!matched.length) {
      toast.add({ title: t('linkgrabber.nzb.drop.none_matched'), color: 'warning', icon: 'i-lucide-file-x' })
      return
    }
    requestFileImport(matched)
    void router.push('/linkgrabber')
  })

  // Esc-cancelled drags fire no dragleave, so the depth counter is reset unconditionally here too.
  useEventListener(window, 'dragend', reset)
  useEventListener(window, 'blur', reset)

  return { dropActive }
}
