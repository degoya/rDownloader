<script setup lang="ts">
/**
 * Tools: where the external helpers are looked up, what was found, and the managed versions.
 *
 * Until RD-110-29 the three sat at the foot of the Interface page, under a header that said
 * "this browser only" about settings that belong to the service. The vendor folder and the
 * managed-tools switches are settings-document fields, so this page shows the save bar; the
 * status and the managed versions load and act on their own.
 */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsManagedTools from '@/components/settings/SettingsManagedTools.vue'
import SettingsToolStatus from '@/components/settings/SettingsToolStatus.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()

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
    <section class="border border-muted bg-default p-5">
      <SectionHeader :eyebrow="t('settings.vendor.directory.eyebrow')" :title="t('settings.vendor.directory.title')" level="sub" />
      <UFormField
        class="mt-4"
        :label="t('settings.vendor.directory.label')"
        :description="t('settings.vendor.directory.description')"
      >
        <UInput v-model="settings.vendor_directory" icon="i-lucide-folder-tree" :placeholder="t('settings.vendor.directory.placeholder')" class="w-full font-mono" />
      </UFormField>
      <div class="mt-4 flex items-start justify-between gap-5 border-t border-muted pt-4">
        <div>
          <p class="text-sm font-medium text-highlighted">{{ t('settings.managed_tools.enabled_label') }}</p>
          <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.managed_tools.enabled_description') }}</p>
        </div>
        <USwitch v-model="settings.managed_tools_enabled" :aria-label="t('settings.managed_tools.enabled_label')" />
      </div>
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
    </section>

    <SettingsToolStatus />
    <SettingsManagedTools />
  </div>
</template>
