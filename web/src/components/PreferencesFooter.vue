<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { useTheme, type ThemeMode } from '@/composables/useTheme'
import { SUPPORTED_LOCALES, setLocale, type AppLocale } from '@/i18n'

const props = defineProps<{ collapsed?: boolean }>()
const { t, locale } = useI18n()
const { theme } = useTheme()

const localeItems = computed(() => SUPPORTED_LOCALES.map(code => ({ label: t(`common.locales.${code}`), value: code })))
const themeItems = computed(() => (['system', 'light', 'dark'] as ThemeMode[]).map(value => ({
  label: t(`common.preferences.theme_${value}`),
  value,
  icon: value === 'light' ? 'i-lucide-sun' : value === 'dark' ? 'i-lucide-moon' : 'i-lucide-monitor'
})))
const localeModel = computed({
  get: () => locale.value as AppLocale,
  set: (value: AppLocale) => setLocale(value)
})
const themeIcon = computed(() => themeItems.value.find(item => item.value === theme.value)?.icon ?? 'i-lucide-monitor')
const menuItems = computed(() => [
  localeItems.value.map(item => ({ label: item.label, icon: 'i-lucide-languages', onSelect: () => setLocale(item.value), checked: item.value === locale.value, type: 'checkbox' as const })),
  themeItems.value.map(item => ({ label: item.label, icon: item.icon, onSelect: () => { theme.value = item.value }, checked: item.value === theme.value, type: 'checkbox' as const }))
])
</script>

<template>
  <!-- One menu for both choices on the rail, so its face is a gear — not the theme icon, which
       promised only half of what is behind it, and not the navigation's settings icon either,
       because this is not the settings page (RD-110-32). -->
  <div v-if="props.collapsed" class="flex justify-center py-1">
    <UDropdownMenu :items="menuItems">
      <UButton
        icon="i-lucide-settings-2"
        size="xs"
        color="neutral"
        variant="ghost"
        :aria-label="t('common.preferences.language_and_theme')"
        :title="t('common.preferences.language_and_theme')"
      />
    </UDropdownMenu>
  </div>
  <!-- One per row: side by side each select kept 20-30 px for its value, and "English",
       "Français" and "Dunkel" were cut to their first letters (RD-120-53). No inset of their
       own: they span the footer's width, edge to edge with the separator above (RD-120-61). -->
  <div v-else class="grid grid-cols-1 gap-1 pb-2">
    <USelect v-model="localeModel" :items="localeItems" value-key="value" size="xs" icon="i-lucide-languages" :aria-label="t('common.preferences.language')" />
    <USelect v-model="theme" :items="themeItems" value-key="value" size="xs" :icon="themeIcon" :aria-label="t('common.preferences.theme')" />
  </div>
</template>
