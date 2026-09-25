<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import { useNotifications } from '@/composables/useNotifications'
import { useTheme, type ThemeMode } from '@/composables/useTheme'
import { SUPPORTED_LOCALES, setLocale, type AppLocale } from '@/i18n'
import { BYTE_UNIT_STEPS } from '@/utils/byteDisplay'

const settings = defineModel<Settings>({ required: true })

/**
 * Binary is the IEC ladder every earlier version used; decimal is what drive manufacturers and
 * most download tools print. The example in each label is the same size shown both ways, which
 * says more than the unit names do.
 */
const byteDisplayItems = computed(() => [
  { value: 'binary', label: t('settings.appearance.byte_display.binary') },
  { value: 'decimal', label: t('settings.appearance.byte_display.decimal') }
])

/**
 * Automatic scaling first, then the ladder from small to large. The magnitudes are named, not
 * the units, because the unit names come from the choice above it (RD-106-14).
 */
const byteUnitItems = computed(() => ['auto', ...BYTE_UNIT_STEPS].map(value => ({
  value,
  label: t(`settings.appearance.byte_unit.${value}`)
})))
const { t, locale } = useI18n()
const { theme } = useTheme()
const notifications = useNotifications()
const notificationsDenied = ref(false)

const localeItems = computed(() => SUPPORTED_LOCALES.map(code => ({ label: t(`common.locales.${code}`), value: code })))
const themeItems = computed(() => (['system', 'light', 'dark'] as ThemeMode[]).map(value => ({
  label: t(`common.preferences.theme_${value}`),
  value
})))
const localeModel = computed({
  get: () => locale.value as AppLocale,
  set: (value: AppLocale) => setLocale(value)
})

const notificationsModel = computed({
  get: () => notifications.enabled.value,
  set: (value: boolean) => void toggleNotifications(value)
})

async function toggleNotifications(value: boolean): Promise<void> {
  if (!value) {
    notificationsDenied.value = false
    notifications.disable()
    return
  }
  notificationsDenied.value = !(await notifications.enable())
}
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.interface.eyebrow')"
        :title="t('settings.headers.interface.title')"
        :description="t('settings.headers.interface.description')"
        level="page"
      />
    </header>
    <section class="border border-muted bg-default p-5">
      <SectionHeader
        :eyebrow="t('settings.appearance.eyebrow')"
        :title="t('settings.appearance.title')"
        :description="t('settings.appearance.description')"
        level="sub"
      />
      <div class="mt-4 grid gap-3">
        <UFormField :label="t('common.preferences.language')" :description="t('settings.appearance.language_description')">
          <USelect v-model="localeModel" :items="localeItems" value-key="value" icon="i-lucide-languages" class="w-full" />
        </UFormField>
        <UFormField :label="t('common.preferences.theme')" :description="t('settings.appearance.theme_description')">
          <USelect v-model="theme" :items="themeItems" value-key="value" icon="i-lucide-sun-moon" class="w-full" />
        </UFormField>
        <UFormField :label="t('settings.appearance.byte_display.label')" :description="t('settings.appearance.byte_display.description')">
          <USelect v-model="settings.byte_display" :items="byteDisplayItems" value-key="value" icon="i-lucide-hard-drive" class="w-full" />
        </UFormField>
        <UFormField :label="t('settings.appearance.byte_unit.label')" :description="t('settings.appearance.byte_unit.description')">
          <USelect v-model="settings.byte_unit" :items="byteUnitItems" value-key="value" icon="i-lucide-ruler" class="w-full" />
        </UFormField>
      </div>
      <div class="mt-4 flex items-start justify-between gap-5 border-t border-muted pt-4">
        <div>
          <p class="text-sm font-medium text-highlighted">{{ t('settings.appearance.title_status.label') }}</p>
          <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.appearance.title_status.description') }}</p>
        </div>
        <USwitch v-model="settings.title_status_enabled" :aria-label="t('settings.appearance.title_status.label')" />
      </div>
      <div class="mt-4 flex items-start justify-between gap-5 border-t border-muted pt-4">
        <div>
          <p class="text-sm font-medium text-highlighted">{{ t('settings.notifications.label') }}</p>
          <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.notifications.description') }}</p>
          <p v-if="!notifications.supported" class="mt-1 text-xs leading-5 text-warning">{{ t('settings.notifications.unsupported') }}</p>
          <p v-else-if="notificationsDenied || notifications.permission.value === 'denied'" class="mt-1 text-xs leading-5 text-warning">{{ t('settings.notifications.denied') }}</p>
        </div>
        <USwitch v-model="notificationsModel" :disabled="!notifications.supported" :aria-label="t('settings.notifications.label')" />
      </div>
    </section>

  </div>
</template>
