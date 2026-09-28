<script setup lang="ts">
/**
 * Switching whole transfer services off.
 *
 * A switched-off service refuses matching links at intake and blocks whatever it already had
 * queued, with a visible reason — rather than leaving rows waiting for a runner that will
 * never pick them up.
 */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
</script>

<template>
  <div class="space-y-4">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.services.eyebrow')"
        :title="t('settings.headers.services.title')"
        :description="t('settings.headers.services.description')"
        level="page"
      />
    </header>
    <section class="border border-muted bg-default p-5">
      <div class="flex flex-col gap-4">
        <div>
          <SectionHeader
            :eyebrow="t('settings.services.eyebrow')"
            :title="t('settings.services.title')"
            :description="t('settings.services.description')"
            level="sub"
          />
        </div>
        <UFormField :label="t('settings.services.torrent.label')" :description="t('settings.services.torrent.description')" orientation="horizontal">
          <USwitch
            v-model="settings.torrent_service_enabled"
            data-testid="service-torrent"
          />
        </UFormField>
        <UFormField :label="t('settings.services.usenet.label')" :description="t('settings.services.usenet.description')" orientation="horizontal">
          <USwitch
            v-model="settings.usenet_service_enabled"
            data-testid="service-usenet"
          />
        </UFormField>
        <UFormField :label="t('settings.services.media.label')" :description="t('settings.services.media.description')" orientation="horizontal">
          <USwitch
            v-model="settings.media_service_enabled"
            data-testid="service-media"
          />
        </UFormField>
        <UFormField :label="t('settings.services.gallery.label')" :description="t('settings.services.gallery.description')" orientation="horizontal">
          <USwitch
            v-model="settings.gallery_service_enabled"
            data-testid="service-gallery"
          />
        </UFormField>
        <UFormField :label="t('settings.services.recording.label')" :description="t('settings.services.recording.description')" orientation="horizontal">
          <USwitch
            v-model="settings.recording_service_enabled"
            data-testid="service-recording"
          />
        </UFormField>
        <UFormField :label="t('settings.services.remote.label')" :description="t('settings.services.remote.description')" orientation="horizontal">
          <USwitch
            v-model="settings.remote_service_enabled"
            data-testid="service-remote"
          />
        </UFormField>
        <p class="text-xs leading-5 text-muted">{{ t('settings.services.hint') }}</p>
      </div>
    </section>
  </div>
</template>
