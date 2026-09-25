import { useColorMode } from '@vueuse/core'
import { computed, watch } from 'vue'

export type ThemeMode = 'system' | 'light' | 'dark'

const THEME_COLORS = { light: '#f8fafc', dark: '#08151d' } as const

/** Nuxt UI already toggles `html.dark` via `useDark()` with this storage key; we only expose the choice. */
const mode = useColorMode({ storageKey: 'vueuse-color-scheme', emitAuto: true })

watch(() => mode.state.value, (resolved) => {
  const meta = document.querySelector<HTMLMetaElement>('meta[name="theme-color"]')
  if (meta) meta.content = resolved === 'dark' ? THEME_COLORS.dark : THEME_COLORS.light
}, { immediate: true })

export function useTheme() {
  const theme = computed<ThemeMode>({
    get: () => (mode.store.value === 'auto' ? 'system' : mode.store.value) as ThemeMode,
    set: (value) => { mode.value = value === 'system' ? 'auto' : value }
  })
  const resolved = computed(() => mode.state.value)
  return { theme, resolved }
}
