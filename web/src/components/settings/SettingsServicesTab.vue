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
        <div class="flex items-center justify-between gap-5">
          <div>
            <p class="text-sm font-medium text-highlighted">{{ t('settings.services.torrent.label') }}</p>
            <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.services.torrent.description') }}</p>
          </div>
          <USwitch
            v-model="settings.torrent_service_enabled"
            :aria-label="t('settings.services.torrent.label')"
            data-testid="service-torrent"
          />
        </div>
        <div class="flex items-center justify-between gap-5">
          <div>
            <p class="text-sm font-medium text-highlighted">{{ t('settings.services.usenet.label') }}</p>
            <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.services.usenet.description') }}</p>
          </div>
          <USwitch
            v-model="settings.usenet_service_enabled"
            :aria-label="t('settings.services.usenet.label')"
            data-testid="service-usenet"
          />
        </div>
        <div class="flex items-center justify-between gap-5">
          <div>
            <p class="text-sm font-medium text-highlighted">{{ t('settings.services.media.label') }}</p>
            <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.services.media.description') }}</p>
          </div>
          <USwitch
            v-model="settings.media_service_enabled"
            :aria-label="t('settings.services.media.label')"
            data-testid="service-media"
          />
        </div>
        <div class="flex items-center justify-between gap-5">
          <div>
            <p class="text-sm font-medium text-highlighted">{{ t('settings.services.gallery.label') }}</p>
            <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.services.gallery.description') }}</p>
          </div>
          <USwitch
            v-model="settings.gallery_service_enabled"
            :aria-label="t('settings.services.gallery.label')"
            data-testid="service-gallery"
          />
        </div>
        <div class="flex items-center justify-between gap-5">
          <div>
            <p class="text-sm font-medium text-highlighted">{{ t('settings.services.recording.label') }}</p>
            <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.services.recording.description') }}</p>
          </div>
          <USwitch
            v-model="settings.recording_service_enabled"
            :aria-label="t('settings.services.recording.label')"
            data-testid="service-recording"
          />
        </div>
        <div class="flex items-center justify-between gap-5">
          <div>
            <p class="text-sm font-medium text-highlighted">{{ t('settings.services.remote.label') }}</p>
            <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.services.remote.description') }}</p>
          </div>
          <USwitch
            v-model="settings.remote_service_enabled"
            :aria-label="t('settings.services.remote.label')"
            data-testid="service-remote"
          />
        </div>
        <p class="text-xs leading-5 text-muted">{{ t('settings.services.hint') }}</p>
      </div>
    </section>
  </div>
</template>
