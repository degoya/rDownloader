<script setup lang="ts">
/**
 * Every program path the service can be told, in one card on the Tools page (RD-1120-23).
 *
 * They were spread over the cards of the services that run them — yt-dlp and ffmpeg under Media,
 * gallery-dl and streamlink beside them, the RAR tool and rclone under Post-processing — while
 * the lookup that falls back on them, and its result, sat here. The old places link here.
 */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
</script>

<template>
  <UCard as="section" data-settings-anchor="tools.paths" :ui="{ body: 'space-y-4' }">
    <SectionHeader
      :eyebrow="t('settings.tool_paths.eyebrow')"
      :title="t('settings.tool_paths.title')"
      :description="t('settings.tool_paths.description')"
      level="sub"
    />
    <UFormField :label="t('settings.media.ytdlp.label')" :description="t('settings.media.ytdlp.description')">
      <UInput v-model="settings.media_ytdlp_executable" icon="i-lucide-terminal" :placeholder="t('settings.media.ytdlp.placeholder')" class="w-full font-mono" />
    </UFormField>
    <UFormField :label="t('settings.media.ffmpeg.label')" :description="t('settings.media.ffmpeg.description')">
      <UInput v-model="settings.media_ffmpeg_executable" icon="i-lucide-terminal" :placeholder="t('settings.media.ffmpeg.placeholder')" class="w-full font-mono" />
    </UFormField>
    <UFormField :label="t('settings.gallery.executable.label')" :description="t('settings.gallery.executable.description')">
      <UInput v-model="settings.gallery_executable" icon="i-lucide-terminal" placeholder="/usr/bin/gallery-dl" class="w-full font-mono" />
    </UFormField>
    <UFormField :label="t('settings.streams.executable.label')" :description="t('settings.streams.executable.description')">
      <UInput v-model="settings.record_streamlink_executable" icon="i-lucide-terminal" placeholder="/usr/bin/streamlink" class="w-full font-mono" />
    </UFormField>
    <UFormField data-settings-anchor="postprocess.rar_executable" :label="t('settings.postprocess.rar_executable.label')" :description="t('settings.postprocess.rar_executable.description')">
      <UInput v-model="settings.rar_executable" icon="i-lucide-terminal" placeholder="/usr/bin/unrar" class="w-full font-mono" />
    </UFormField>
    <UFormField data-settings-anchor="postprocess.rclone_executable" :label="t('settings.postprocess.rclone_executable.label')" :description="t('settings.postprocess.rclone_executable.description')">
      <UInput v-model="settings.rclone_executable" icon="i-lucide-terminal" placeholder="/usr/bin/rclone" class="w-full font-mono" />
    </UFormField>
  </UCard>
</template>
