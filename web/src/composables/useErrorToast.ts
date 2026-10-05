import { useToast } from '@nuxt/ui/composables'

/**
 * The one error toast: red, the alert icon, a title and an optional description (audit R6).
 *
 * Eleven places spelled it out by hand, and a copy that drifts — another icon, a warning colour
 * — reads as a different kind of message. An empty description is left out rather than shown as
 * an empty line.
 */
export function useErrorToast(): (title: string, description?: string | null) => void {
  const toast = useToast()
  return (title, description) => {
    toast.add({ title, ...(description ? { description } : {}), color: 'error', icon: 'i-lucide-circle-alert' })
  }
}
