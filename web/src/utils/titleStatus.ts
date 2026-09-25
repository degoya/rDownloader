import { ref } from 'vue'

/**
 * Whether the browser tab reports the running state instead of the application name alone
 * (RD-106-07).
 *
 * A `ref` for the same reason `byteDisplay` is one: switching it in the settings has to take
 * effect on the tab that is already open, not after a reload. Defaults to on — a title that
 * says nothing is what every earlier version had, and not having to bring the window forward
 * is the whole point.
 */
export const titleStatus = ref(true)

export function setTitleStatus(value: boolean | null | undefined): void {
  titleStatus.value = value !== false
}
