<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()

/** Mirrors `GallerySettings::default_hosts()` in crates/rd-core/src/gallery.rs; only applied on demand. */
const DEFAULT_GALLERY_HOSTS = [
  'pixiv.net', 'deviantart.com', 'artstation.com', 'flickr.com',
  'danbooru.donmai.us', 'gelbooru.com', 'e621.net', 'rule34.xxx',
  'kemono.cr', 'fanbox.cc', 'redgifs.com', 'imgbox.com'
]
const addedHosts = ref<number | null>(null)

/** Merges the built-in list into the saved tags; never removes a host the user added. */
function addDefaultHosts(): void {
  const known = new Set(settings.value.gallery_hosts.map(host => host.trim().toLowerCase()))
  const missing = DEFAULT_GALLERY_HOSTS.filter(host => !known.has(host))
  if (missing.length) settings.value.gallery_hosts = [...settings.value.gallery_hosts, ...missing]
  addedHosts.value = missing.length
}
</script>

<template>
  <section class="space-y-4 border border-muted bg-default p-5">
    <div>
      <SectionHeader
        :eyebrow="t('settings.gallery.eyebrow')"
        :title="t('settings.gallery.title')"
        :description="t('settings.gallery.description')"
        level="sub"
      />
    </div>
    <UFormField :label="t('settings.gallery.executable.label')" :description="t('settings.gallery.executable.description')">
      <UInput v-model="settings.gallery_executable" icon="i-lucide-terminal" placeholder="/usr/bin/gallery-dl" class="w-full font-mono" />
    </UFormField>
    <UFormField :label="t('settings.gallery.max_parallel.label')" :description="t('settings.gallery.max_parallel.description')">
      <UInput v-model.number="settings.gallery_max_parallel" type="number" min="1" max="8" icon="i-lucide-layers" class="w-full" />
    </UFormField>
    <UFormField :label="t('settings.gallery.hosts.label')" :description="t('settings.gallery.hosts.description')">
      <UInputTags v-model="settings.gallery_hosts" :placeholder="t('settings.gallery.hosts.placeholder')" icon="i-lucide-images" add-on-blur add-on-paste delimiter="," class="w-full font-mono" />
      <div class="mt-2 flex flex-wrap items-center gap-2">
        <UButton
          type="button"
          size="xs"
          color="neutral"
          variant="outline"
          icon="i-lucide-list-plus"
          :label="t('settings.gallery.hosts.add_defaults')"
          @click="addDefaultHosts"
        />
        <span v-if="addedHosts !== null" class="text-xs text-muted">
          {{ addedHosts ? t('settings.media.hosts.defaults_added', { count: addedHosts }, addedHosts) : t('settings.media.hosts.defaults_complete') }}
        </span>
      </div>
      <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.gallery.hosts.hint') }}</p>
    </UFormField>
  </section>
</template>
