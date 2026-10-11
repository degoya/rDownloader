<script setup lang="ts">
/**
 * Tools: where the external helpers are looked up, what was found, and the managed versions.
 *
 * Until RD-110-29 the three sat at the foot of the Interface page, under a header that said
 * "this browser only" about settings that belong to the service. The vendor folder and the
 * managed-tools switches are settings-document fields, so this page shows the save bar; the
 * status and the managed versions load and act on their own. The program paths the services'
 * cards used to carry each are one card here since RD-1120-23.
 *
 * In tabs since RD-1240-26 (owner, 2026-10-10), by what somebody comes to do: see what was found
 * (*Status*, which loads and acts on its own), say where to look (*Paths*: the vendor folder and
 * the program paths), or let the service install versions (*Managed tools*: its switches over the
 * versions). The card that held the vendor folder and the managed switches was split along those
 * two subjects.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsManagedTools from '@/components/settings/SettingsManagedTools.vue'
import SettingsToolPathsCard from '@/components/settings/SettingsToolPathsCard.vue'
import SettingsToolStatus from '@/components/settings/SettingsToolStatus.vue'
import { subTabItems } from '@/composables/useSettingsSubTab'

const settings = defineModel<Settings>({ required: true })
/** Owned by the settings view, which keeps it in the address. */
const activeTab = defineModel<string>('subTab', { default: 'status' })
const { t } = useI18n()
const tabItems = computed(() => subTabItems('tools', t))

/**
 * The tools a compatibility rule can cover, mirroring `rd_tools::compat::RULED_TOOLS`. The
 * backend refuses any other name, so offering a fixed list rather than free text keeps the
 * setting from being saveable in a state that does nothing.
 */
const COMPATIBILITY_OVERRIDE_TOOLS = ['yt-dlp', 'gallery-dl', 'streamlink', 'ffmpeg', 'ffprobe']
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.tools.eyebrow')"
        :title="t('settings.headers.tools.title')"
        :description="t('settings.headers.tools.description')"
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
      <template #status>
        <SettingsToolStatus />
      </template>
      <template #paths>
        <div class="space-y-6">
          <UCard as="section">
            <SectionHeader :eyebrow="t('settings.vendor.directory.eyebrow')" :title="t('settings.vendor.directory.title')" level="sub" />
            <UFormField
              data-settings-anchor="tools.vendor_directory"
              class="mt-4"
              :label="t('settings.vendor.directory.label')"
              :description="t('settings.vendor.directory.description')"
            >
              <UInput v-model="settings.vendor_directory" icon="i-lucide-folder-tree" :placeholder="t('settings.vendor.directory.placeholder')" class="w-full font-mono" />
            </UFormField>
          </UCard>
          <SettingsToolPathsCard v-model="settings" />
        </div>
      </template>
      <template #managed>
        <div class="space-y-6">
          <UCard as="section">
            <UFormField :label="t('settings.managed_tools.enabled_label')" :description="t('settings.managed_tools.enabled_description')" orientation="horizontal">
              <USwitch v-model="settings.managed_tools_enabled" />
            </UFormField>
            <UFormField
              class="mt-4"
              :label="t('settings.managed_tools.manifest_url_label')"
              :description="t('settings.managed_tools.manifest_url_description')"
            >
              <UInput v-model="settings.managed_tools_manifest_url" icon="i-lucide-file-signature" :placeholder="t('settings.managed_tools.manifest_url_placeholder')" class="w-full font-mono" />
            </UFormField>
            <UFormField
              class="mt-4"
              :label="t('settings.managed_tools.overrides_label')"
              :description="t('settings.managed_tools.overrides_description')"
            >
              <USelectMenu
                v-model="settings.tool_compatibility_overrides"
                :items="COMPATIBILITY_OVERRIDE_TOOLS"
                multiple
                class="w-full font-mono"
                :placeholder="t('settings.managed_tools.overrides_placeholder')"
              />
            </UFormField>
          </UCard>
          <SettingsManagedTools />
        </div>
      </template>
    </UTabs>
  </div>
</template>
