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
            <li v-for="section in group.sections" :key="section.value">
              <RouterLink
                :to="`/settings/${section.value}`"
                class="flex h-full items-start gap-3 border border-muted bg-default p-4 transition-colors hover:border-primary hover:bg-elevated/50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                data-settings-card
              >
                <span class="grid size-9 shrink-0 place-items-center bg-elevated text-primary" aria-hidden="true">
                  <UIcon :name="section.icon" class="size-5" />
                </span>
                <span class="min-w-0">
                  <span class="block text-sm font-semibold text-highlighted">{{ t(section.titleKey) }}</span>
                  <span class="mt-1 block text-xs leading-5 text-muted">{{ t(section.descriptionKey) }}</span>
                </span>
              </RouterLink>
            </li>
          </ul>
        </section>
      </div>
    </template>
  </UDashboardPanel>
</template>
