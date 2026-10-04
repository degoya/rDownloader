<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()

/** An empty field means "the built-in service", which the API stores as `null`. */
const endpoint = computed({
  get: () => settings.value.dlc_service_endpoint ?? '',
  set: (value: string) => {
    settings.value.dlc_service_endpoint = value.trim() ? value : null
  }
})

const FORMAT_KEYS = ['one_per_line', 'comments', 'www', 'subdomains'] as const
</script>

<template>
  <div class="space-y-4">
    <section data-settings-anchor="routing.collector" class="grid gap-4 border border-muted bg-default p-5 md:grid-cols-2">
      <div class="space-y-4">
        <div>
          <SectionHeader
            :eyebrow="t('settings.collector.eyebrow')"
            :title="t('settings.collector.title')"
            :description="t('settings.collector.description')"
            level="sub"
          />
        </div>
        <UFormField
          data-settings-anchor="routing.excluded_domains"
          :label="t('settings.collector.excluded_domains.label')"
          :description="t('settings.collector.excluded_domains.description')"
        >
          <UInput v-model="settings.excluded_domains_file" icon="i-lucide-shield-ban" placeholder="/config/excluded_domains.txt" class="w-full font-mono" />
        </UFormField>
      </div>
      <div class="space-y-3 border border-muted bg-elevated p-4">
        <p class="text-sm font-medium text-highlighted">{{ t('settings.collector.format.title') }}</p>
        <ul class="space-y-1.5 text-xs leading-5 text-muted">
          <li v-for="key in FORMAT_KEYS" :key="key" class="flex items-start gap-2">
            <UIcon name="i-lucide-dot" class="mt-0.5 size-4 shrink-0 text-primary" />
            <span>{{ t(`settings.collector.format.${key}`) }}</span>
          </li>
        </ul>
        <pre class="overflow-x-auto border border-muted bg-default p-3 font-mono text-[11px] leading-5 text-muted"># {{ t('settings.collector.format.sample_comment') }}
example.com
www.tracker.example
ads.example.org</pre>
      </div>
    </section>
    <section data-settings-anchor="routing.dlc" class="border border-muted bg-default p-5">
      <div class="space-y-4">
        <div>
          <SectionHeader
            :eyebrow="t('settings.collector.dlc.eyebrow')"
            :title="t('settings.collector.dlc.title')"
            :description="t('settings.collector.dlc.description')"
            level="sub"
          />
        </div>
        <UFormField :label="t('settings.collector.dlc.enabled.label')" orientation="horizontal" class="border-t border-muted pt-4">
          <template #description>
            {{ t('settings.collector.dlc.enabled.description') }}
            <span v-if="settings.dlc_service_enabled" class="mt-1 block text-warning">{{ t('settings.collector.dlc.enabled.warning') }}</span>
          </template>
          <USwitch v-model="settings.dlc_service_enabled" />
        </UFormField>
        <UFormField
          :label="t('settings.collector.dlc.endpoint.label')"
          :description="t('settings.collector.dlc.endpoint.description')"
        >
          <UInput
            v-model="endpoint"
            icon="i-lucide-link"
            :disabled="!settings.dlc_service_enabled"
            placeholder="http://service.jdownloader.org/dlcrypt/service.php"
            class="w-full font-mono"
          />
        </UFormField>
      </div>
    </section>

    <section data-settings-anchor="routing.indexer_images" class="border border-muted bg-default p-5">
      <div class="space-y-4">
        <div>
          <SectionHeader
            :eyebrow="t('settings.collector.indexer_images.eyebrow')"
            :title="t('settings.collector.indexer_images.title')"
            :description="t('settings.collector.indexer_images.description')"
            level="sub"
          />
        </div>
        <UFormField :label="t('settings.collector.indexer_images.enabled.label')" :description="t('settings.collector.indexer_images.enabled.description')" orientation="horizontal" class="border-t border-muted pt-4">
          <USwitch v-model="settings.subscription_item_images_enabled" />
        </UFormField>
      </div>
    </section>

    <section data-settings-anchor="routing.nzb_hand_over" class="border border-muted bg-default p-5">
      <div class="space-y-4">
        <div>
          <SectionHeader
            :eyebrow="t('settings.collector.nzb_hand_over.eyebrow')"
            :title="t('settings.collector.nzb_hand_over.title')"
            :description="t('settings.collector.nzb_hand_over.description')"
            level="sub"
          />
        </div>
        <UFormField :label="t('settings.collector.nzb_hand_over.linkgrabber.label')" :description="t('settings.collector.nzb_hand_over.linkgrabber.description')" orientation="horizontal" class="border-t border-muted pt-4">
          <USwitch v-model="settings.nzb_hand_over_linkgrabber_enabled" />
        </UFormField>
        <UFormField :label="t('settings.collector.nzb_hand_over.downloads.label')" :description="t('settings.collector.nzb_hand_over.downloads.description')" orientation="horizontal" class="border-t border-muted pt-4">
          <USwitch v-model="settings.nzb_hand_over_downloads_enabled" />
        </UFormField>
      </div>
    </section>
  </div>
</template>
