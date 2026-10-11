<script setup lang="ts">
/**
 * *Post-processing › Unpacking* (RD-1240-26): the pipeline's level and how archives are opened.
 * The card keeps the page's anchor and header; repair and cleanup, the package names, the malware
 * scan and the scripts with the upload are cards of their own tabs.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { PostprocessLevel, Settings } from '@/api/types'
import { GIB, byteModel, postprocessLevelItems } from '@/utils/format'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsCrossLink from '@/components/settings/SettingsCrossLink.vue'
import NumberWithUnit from '@/components/NumberWithUnit.vue'
import { DECIMAL, WHOLE } from '@/utils/numberInput'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const rarToolItems = [
  { label: 'unrar', value: 'unrar' },
  { label: '7z', value: '7z' }
]
const levelItems = computed(() => postprocessLevelItems(false))
const defaultLevel = computed({
  get: () => settings.value.default_level ?? 'unpack',
  set: (value: string) => { settings.value.default_level = value as PostprocessLevel }
})

// Obligatory and above zero: an emptied field falls back to the default of 100 GiB.
const archiveMaxGiB = byteModel(
  () => settings.value.archive_max_uncompressed_bytes,
  (raw) => { settings.value.archive_max_uncompressed_bytes = raw ?? String(100 * GIB) },
  GIB,
  String(100 * GIB)
)
</script>

<template>
  <UCard as="section" data-settings-anchor="postprocess.defaults" :ui="{ body: 'space-y-4' }">
    <div>
      <SectionHeader
        :eyebrow="t('settings.postprocess.eyebrow')"
        :title="t('settings.postprocess.title')"
        :description="t('settings.postprocess.description')"
        level="sub"
      />
    </div>
    <UFormField :label="t('settings.postprocess.default_level.label')" :description="t('settings.postprocess.default_level.description')">
      <USelect v-model="defaultLevel" :items="levelItems" value-key="value" icon="i-lucide-workflow" class="w-full" />
    </UFormField>
    <UFormField :label="t('settings.postprocess.recursive_unpack.label')" :description="t('settings.postprocess.recursive_unpack.description')" orientation="horizontal">
      <USwitch v-model="settings.recursive_unpack" />
    </UFormField>
    <UFormField data-settings-anchor="postprocess.unpack_to_subfolder" :label="t('settings.postprocess.unpack_to_subfolder.label')" :description="t('settings.postprocess.unpack_to_subfolder.description')" orientation="horizontal">
      <USwitch v-model="settings.unpack_to_subfolder" data-testid="unpack-to-subfolder" />
    </UFormField>
    <UFormField data-settings-anchor="postprocess.unwrap_package_folder" :label="t('settings.postprocess.unwrap_package_folder.label')" :description="t('settings.postprocess.unwrap_package_folder.description')" orientation="horizontal">
      <USwitch v-model="settings.unwrap_package_folder" data-testid="unwrap-package-folder" />
    </UFormField>
    <UFormField data-settings-anchor="postprocess.direct_unpack" :label="t('settings.postprocess.direct_unpack.label')" :description="t('settings.postprocess.direct_unpack.description')" orientation="horizontal">
      <USwitch v-model="settings.direct_unpack" data-testid="direct-unpack" />
    </UFormField>
    <UFormField :label="t('settings.postprocess.pause.label')" :description="t('settings.postprocess.pause.description')" orientation="horizontal">
      <USwitch v-model="settings.pause_during_postprocess" />
    </UFormField>
    <UFormField data-settings-anchor="postprocess.passwords_file" :label="t('settings.postprocess.passwords_file.label')" :description="t('settings.postprocess.passwords_file.description')">
      <UInput v-model="settings.passwords_file" icon="i-lucide-key-round" placeholder="/config/passwords.txt" class="w-full font-mono" />
    </UFormField>
    <div class="grid gap-3 sm:grid-cols-2">
      <UFormField :label="t('settings.postprocess.max_files')">
        <UInputNumber v-model="settings.archive_max_files" required :min="1" :max="1000000" :format-options="WHOLE" class="w-full" />
      </UFormField>
      <UFormField :label="t('settings.postprocess.max_bytes')">
        <NumberWithUnit v-model="archiveMaxGiB" unit="GiB" :min="0.01" :format-options="DECIMAL" :step-snapping="false" class="w-full" data-testid="archive-max-size" />
      </UFormField>
    </div>
    <UFormField :label="t('settings.postprocess.rar_tool')">
      <USelect v-model="settings.rar_tool" :items="rarToolItems" class="w-full" />
    </UFormField>
    <SettingsCrossLink class="-mt-3" anchor="postprocess.rar_executable" :lead="t('settings.cross_link.program_path')" />
  </UCard>
</template>
