<script setup lang="ts">
/**
 * The settings entry page (RD-110-29): one card per page, grouped by the six rubrics of
 * `settingsSections.ts`, each card carrying the page's own title and description.
 *
 * `/settings` used to redirect to the system page, so the way into the settings led to a
 * status page with sixteen siblings hidden in the sidebar. The overview reads the same table
 * the sidebar reads, so both always show the same rubrics with the same pages in them; each
 * card is a link, so the keyboard reaches it the way it reaches the sidebar.
 */
import { useI18n } from 'vue-i18n'

import SectionHeader from '@/components/SectionHeader.vue'
import { SETTINGS_SECTION_GROUPS } from '@/settingsSections'

const { t } = useI18n()
</script>

<template>
  <UDashboardPanel id="settings">
    <template #header>
      <UDashboardNavbar :title="t('settings.title')">
        <template #leading><UDashboardSidebarCollapse /></template>
      </UDashboardNavbar>
    </template>
    <template #body>
      <div class="w-full space-y-8 pt-4" data-tour="settings-tabs">
        <header>
          <SectionHeader
            :eyebrow="t('settings.overview.eyebrow')"
            :title="t('settings.overview.title')"
            :description="t('settings.overview.description')"
            level="page"
          />
        </header>
        <section
          v-for="group in SETTINGS_SECTION_GROUPS"
          :key="group.value"
          :aria-labelledby="`settings-group-${group.value}`"
          data-settings-group
        >
          <h3 :id="`settings-group-${group.value}`" class="eyebrow mb-3">{{ t(group.labelKey) }}</h3>
          <ul class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
            <li v-for="section in group.sections" :key="section.value" data-settings-card>
              <UPageCard
                :to="`/settings/${section.value}`"
                :title="t(section.titleKey)"
                :description="t(section.descriptionKey)"
                variant="soft"
                class="h-full"
                :ui="{ container: 'p-4 sm:p-4', wrapper: 'flex-row gap-3', leading: 'mb-0', body: 'min-w-0', title: 'text-sm', description: 'text-xs leading-5 text-muted' }"
              >
                <template #leading>
                  <UAvatar :icon="section.icon" color="primary" size="lg" />
                </template>
              </UPageCard>
            </li>
          </ul>
        </section>
      </div>
    </template>
  </UDashboardPanel>
</template>
