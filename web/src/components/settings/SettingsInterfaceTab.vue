<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import { useNotifications } from '@/composables/useNotifications'
import { useTheme, type ThemeMode } from '@/composables/useTheme'
import { languageItems, setLocale, type AppLocale } from '@/i18n'
import { BYTE_UNIT_STEPS } from '@/utils/byteDisplay'
import SettingsCrossLink from '@/components/settings/SettingsCrossLink.vue'

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

const localeItems = computed(() => languageItems(t))
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
    <UCard as="section" data-settings-anchor="interface.appearance">
      <SectionHeader
        :eyebrow="t('settings.appearance.eyebrow')"
        :title="t('settings.appearance.title')"
        :description="t('settings.appearance.description')"
        level="sub"
      />
      <div class="mt-4 grid gap-3">
        <UFormField data-settings-anchor="interface.language" :label="t('common.preferences.language')" :description="t('settings.appearance.language_description')">
          <USelect v-model="localeModel" :items="localeItems" value-key="value" icon="i-lucide-languages" class="w-full" />
        </UFormField>
        <UFormField data-settings-anchor="interface.theme" :label="t('common.preferences.theme')" :description="t('settings.appearance.theme_description')">
          <USelect v-model="theme" :items="themeItems" value-key="value" icon="i-lucide-sun-moon" class="w-full" />
        </UFormField>
      </div>
      <UFormField data-settings-anchor="interface.browser_notifications" :label="t('settings.notifications.label')" orientation="horizontal" class="mt-4 border-t border-muted pt-4">
        <template #description>
          {{ t('settings.notifications.description') }}
          <span v-if="!notifications.supported" class="mt-1 block text-warning">{{ t('settings.notifications.unsupported') }}</span>
          <span v-else-if="notificationsDenied || notifications.permission.value === 'denied'" class="mt-1 block text-warning">{{ t('settings.notifications.denied') }}</span>
        </template>
        <USwitch v-model="notificationsModel" :disabled="!notifications.supported" />
      </UFormField>
      <SettingsCrossLink class="mt-2" anchor="notifications.targets" />
    </UCard>

    <!-- Fields of the settings document: they hold in every browser, so they are not under the
         "this browser only" card above (RD-1120-23). -->
    <UCard as="section" data-settings-anchor="interface.display">
      <SectionHeader
        :eyebrow="t('settings.display.eyebrow')"
        :title="t('settings.display.title')"
        :description="t('settings.display.description')"
        level="sub"
      />
      <div class="mt-4 grid gap-3">
        <UFormField data-settings-anchor="interface.byte_display" :label="t('settings.appearance.byte_display.label')" :description="t('settings.appearance.byte_display.description')">
          <USelect v-model="settings.byte_display" :items="byteDisplayItems" value-key="value" icon="i-lucide-hard-drive" class="w-full" />
        </UFormField>
        <UFormField :label="t('settings.appearance.byte_unit.label')" :description="t('settings.appearance.byte_unit.description')">
          <USelect v-model="settings.byte_unit" :items="byteUnitItems" value-key="value" icon="i-lucide-ruler" class="w-full" />
        </UFormField>
      </div>
      <UFormField data-settings-anchor="interface.title_status" :label="t('settings.appearance.title_status.label')" :description="t('settings.appearance.title_status.description')" orientation="horizontal" class="mt-4 border-t border-muted pt-4">
        <USwitch v-model="settings.title_status_enabled" />
      </UFormField>
      <UFormField data-settings-anchor="interface.indexer_images" :label="t('settings.collector.indexer_images.enabled.label')" orientation="horizontal" class="mt-4 border-t border-muted pt-4">
        <template #description>
          {{ t('settings.collector.indexer_images.description') }}
          <span class="mt-1 block">{{ t('settings.collector.indexer_images.enabled.description') }}</span>
        </template>
        <USwitch v-model="settings.subscription_item_images_enabled" />
      </UFormField>
      <USeparator class="mt-4" />
      <div data-settings-anchor="interface.nzb_hand_over" class="mt-4 space-y-4">
        <div>
          <SectionHeader
            :eyebrow="t('settings.collector.nzb_hand_over.eyebrow')"
            :title="t('settings.collector.nzb_hand_over.title')"
            :description="t('settings.collector.nzb_hand_over.description')"
            level="sub"
          />
          <SettingsCrossLink class="mt-2" anchor="accounts.list" />
        </div>
        <UFormField :label="t('settings.collector.nzb_hand_over.linkgrabber.label')" :description="t('settings.collector.nzb_hand_over.linkgrabber.description')" orientation="horizontal">
          <USwitch v-model="settings.nzb_hand_over_linkgrabber_enabled" />
        </UFormField>
        <UFormField :label="t('settings.collector.nzb_hand_over.downloads.label')" :description="t('settings.collector.nzb_hand_over.downloads.description')" orientation="horizontal">
          <USwitch v-model="settings.nzb_hand_over_downloads_enabled" />
        </UFormField>
      </div>
      <USeparator class="mt-4" />
      <!-- The default for a package nobody has opened or closed; what was is kept per browser (RD-1170-01). -->
      <div data-settings-anchor="interface.package_groups" class="mt-4 space-y-4">
        <SectionHeader
          :eyebrow="t('settings.package_groups.eyebrow')"
          :title="t('settings.package_groups.title')"
          :description="t('settings.package_groups.description')"
          level="sub"
        />
        <UFormField :label="t('settings.package_groups.downloads.label')" :description="t('settings.package_groups.downloads.description')" orientation="horizontal">
          <USwitch v-model="settings.downloads_packages_closed_by_default" />
        </UFormField>
        <UFormField :label="t('settings.package_groups.linkgrabber.label')" :description="t('settings.package_groups.linkgrabber.description')" orientation="horizontal">
          <USwitch v-model="settings.linkgrabber_packages_closed_by_default" />
        </UFormField>
      </div>
    </UCard>
  </div>
</template>
