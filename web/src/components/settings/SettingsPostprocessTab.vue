<script setup lang="ts">
/**
 * Post-processing: the page header over the pipeline card, moved out of the view (RD-110-29).
 *
 * In tabs since RD-1240-26 (owner, 2026-10-10): the one card held some twenty-five fields. The
 * malware scan and the package names are tabs of their own, as the owner asked; the rest is split
 * in the order a package meets it — unpacked, repaired and cleaned up, then scripts and upload.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsMalwareScan from '@/components/SettingsMalwareScan.vue'
import SettingsPackageNameRules from '@/components/SettingsPackageNameRules.vue'
import SettingsPostprocessCard from '@/components/SettingsPostprocessCard.vue'
import SettingsPostprocessDeliveryCard from '@/components/settings/SettingsPostprocessDeliveryCard.vue'
import SettingsPostprocessRepairCard from '@/components/settings/SettingsPostprocessRepairCard.vue'
import { subTabItems } from '@/composables/useSettingsSubTab'

const settings = defineModel<Settings>({ required: true })
/** Owned by the settings view, which keeps it in the address. */
const activeTab = defineModel<string>('subTab', { default: 'unpack' })
const { t } = useI18n()
const tabItems = computed(() => subTabItems('postprocess', t))
</script>

<template>
  <div class="space-y-4">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.postprocess.eyebrow')"
        :title="t('settings.headers.postprocess.title')"
        :description="t('settings.headers.postprocess.description')"
        level="page"
      />
    </header>
    <UTabs
      v-model="activeTab"
      :items="tabItems"
      :unmount-on-hide="false"
      variant="pill"
      class="w-full"
    >
      <template #unpack>
        <SettingsPostprocessCard :model-value="settings" />
      </template>
      <template #repair>
        <SettingsPostprocessRepairCard :model-value="settings" />
      </template>
      <template #names>
        <SettingsPackageNameRules :model-value="settings" />
      </template>
      <template #malware>
        <SettingsMalwareScan :model-value="settings" />
      </template>
      <template #delivery>
        <SettingsPostprocessDeliveryCard :model-value="settings" />
      </template>
    </UTabs>
  </div>
</template>
