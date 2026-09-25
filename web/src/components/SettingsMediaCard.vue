<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { MediaStatus, MediaToolStatus, Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const status = ref<MediaStatus | null>(null)
const statusError = ref(false)

const VARIANTS: { value: string, key: string }[] = [
  { value: 'best', key: 'video_best' },
  { value: '2160p', key: 'video_2160' },
  { value: '1440p', key: 'video_1440' },
  { value: '1080p', key: 'video_1080' },
  { value: '720p', key: 'video_720' },
  { value: '480p', key: 'video_480' },
  { value: 'audio_mp3', key: 'audio_mp3' }
]
const variantItems = computed(() => VARIANTS.map(({ value, key }) => ({ label: t(`settings.media.variants.${key}`), value })))
/**
 * Mirrors `TEMPLATE_FIELDS` in crates/rd-files/src/template.rs. Listed rather than fetched:
 * the per-link editor gets the authoritative list with its preview, and this is a hint on a
 * settings page. An invalid template is refused by the server on save either way.
 */
const TEMPLATE_FIELDS = '{title} {uploader} {upload_date} {upload_year} {extractor} {id} {resolution} {ext}'
const tools = computed<MediaToolStatus[]>(() => status.value ? [status.value.ytdlp, status.value.ffmpeg] : [])

/** Mirrors `MediaSettings::default_hosts()` in crates/rd-core/src/media/mod.rs; only applied on demand. */
const DEFAULT_MEDIA_HOSTS = [
  'youtube.com', 'youtu.be', 'm.youtube.com', 'music.youtube.com',
  'vimeo.com', 'dailymotion.com', 'twitch.tv', 'rumble.com', 'odysee.com', 'bitchute.com',
  'peertube.tv', 'streamable.com', 'veoh.com', 'dumpert.nl',
  'tiktok.com', 'instagram.com', 'facebook.com', 'twitter.com', 'x.com', 'reddit.com',
  'tumblr.com', 'snapchat.com', 'vk.com', 'ok.ru', 'bilibili.com', 'nicovideo.jp',
  'soundcloud.com', 'bandcamp.com', 'mixcloud.com', 'audiomack.com',
  'ardmediathek.de', 'zdf.de', 'arte.tv', '3sat.de', 'kika.de', 'dw.com', 'orf.at', 'srf.ch',
  'bbc.co.uk', 'channel4.com', 'france.tv', 'rai.it', 'rtve.es', 'npo.nl',
  'cnn.com', 'nytimes.com', 'theguardian.com', 'heise.de', 'spiegel.de', 'ted.com',
  'archive.org', 'imgur.com', '9gag.com'
]
const addedHosts = ref<number | null>(null)

/** Merges the built-in list into the saved tags; never removes a host the user added. */
function addDefaultHosts(): void {
  const known = new Set(settings.value.media_hosts.map(host => host.trim().toLowerCase()))
  const missing = DEFAULT_MEDIA_HOSTS.filter(host => !known.has(host))
  if (missing.length) settings.value.media_hosts = [...settings.value.media_hosts, ...missing]
  addedHosts.value = missing.length
}

onMounted(async () => {
  const response = await api.GET('/api/v1/system/media')
  if (response.data) status.value = response.data
  else statusError.value = true
})

function toolDetail(tool: MediaToolStatus): string {
  return [tool.version, tool.path].filter(Boolean).join(' · ')
}
</script>

<template>
  <section class="space-y-4 border border-muted bg-default p-5">
    <div>
      <SectionHeader
        :eyebrow="t('settings.media.eyebrow')"
        :title="t('settings.media.title')"
        :description="t('settings.media.description')"
        level="sub"
      />
    </div>
    <UFormField :label="t('settings.media.ytdlp.label')" :description="t('settings.media.ytdlp.description')">
      <UInput v-model="settings.media_ytdlp_executable" icon="i-lucide-terminal" :placeholder="t('settings.media.ytdlp.placeholder')" class="w-full font-mono" />
    </UFormField>
    <UFormField :label="t('settings.media.ffmpeg.label')" :description="t('settings.media.ffmpeg.description')">
      <UInput v-model="settings.media_ffmpeg_executable" icon="i-lucide-terminal" :placeholder="t('settings.media.ffmpeg.placeholder')" class="w-full font-mono" />
    </UFormField>
    <div class="space-y-2 border border-muted p-3">
      <div v-for="tool in tools" :key="tool.name" class="flex min-w-0 items-center gap-2">
        <UIcon :name="tool.path ? 'i-lucide-circle-check' : 'i-lucide-circle-alert'" class="size-4 shrink-0" :class="tool.path ? 'text-success' : 'text-warning'" />
        <span class="w-14 shrink-0 font-mono text-xs text-highlighted">{{ tool.name }}</span>
        <span v-if="tool.path" class="min-w-0 truncate font-mono text-[11px] text-muted" :title="toolDetail(tool)">{{ toolDetail(tool) }}</span>
        <span v-else class="text-xs text-warning">{{ t('settings.media.status.not_found') }}</span>
      </div>
      <p v-if="statusError" class="text-xs text-error">{{ t('settings.media.status.unavailable') }}</p>
      <p v-else-if="!status" class="text-xs text-muted">{{ t('settings.media.status.loading') }}</p>
      <p class="text-xs leading-5 text-muted">{{ t('settings.media.status.hint') }}</p>
    </div>
    <UFormField :label="t('settings.media.default_variant.label')" :description="t('settings.media.default_variant.description')">
      <USelect v-model="settings.media_default_variant" :items="variantItems" value-key="value" icon="i-lucide-clapperboard" class="w-full" />
    </UFormField>
    <UFormField :label="t('settings.media.output_template.label')" :description="t('settings.media.output_template.description')">
      <UInput
        :model-value="settings.media_output_template ?? ''"
        :placeholder="t('settings.media.output_template.placeholder')"
        icon="i-lucide-folder-tree"
        class="w-full font-mono"
        @update:model-value="(value: string) => settings.media_output_template = value.trim() || null"
      />
      <p class="mt-1 text-xs text-muted">{{ t('settings.media.output_template.fields', { fields: TEMPLATE_FIELDS }) }}</p>
    </UFormField>
    <UFormField :label="t('settings.media.hosts.label')" :description="t('settings.media.hosts.description')">
      <UInputTags v-model="settings.media_hosts" :placeholder="t('settings.media.hosts.placeholder')" icon="i-lucide-globe" add-on-blur add-on-paste delimiter="," class="w-full font-mono" />
      <div class="mt-2 flex flex-wrap items-center gap-2">
        <UButton
          type="button"
          size="xs"
          color="neutral"
          variant="outline"
          icon="i-lucide-list-plus"
          :label="t('settings.media.hosts.add_defaults')"
          @click="addDefaultHosts"
        />
        <span v-if="addedHosts !== null" class="text-xs text-muted">
          {{ addedHosts ? t('settings.media.hosts.defaults_added', { count: addedHosts }, addedHosts) : t('settings.media.hosts.defaults_complete') }}
        </span>
      </div>
      <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.media.hosts.add_defaults_hint') }}</p>
    </UFormField>
    <UFormField :label="t('settings.media.max_parallel.label')" :description="t('settings.media.max_parallel.description')">
      <UInput v-model.number="settings.media_max_parallel" type="number" min="1" max="8" icon="i-lucide-layers" class="w-full" />
    </UFormField>
    <UFormField :label="t('settings.media.check_timeout.label')" :description="t('settings.media.check_timeout.description')">
      <UInput v-model.number="settings.media_check_timeout_seconds" type="number" min="5" max="600" icon="i-lucide-timer" class="w-full">
        <template #trailing><span class="font-mono text-xs text-muted">s</span></template>
      </UInput>
    </UFormField>
  </section>
</template>
