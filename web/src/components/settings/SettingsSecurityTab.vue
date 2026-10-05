<script setup lang="ts">
/**
 * Security: the administrator password, the second factor and the passkeys side by side on
 * large screens, how long a sign-in lasts, the sessions, and the reverse proxy this service
 * may believe. The page had no header until RD-110-29, no way to change the password until
 * RD-120-22, and a fixed twelve-hour sign-in until RD-130-09. Six cards made it three tabs in
 * RD-180-15: how one signs in, how long a sign-in lasts and where, and the reverse proxy. The
 * identity provider (RD-190-15) is one more way to sign in, under the first.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import { subTabItems } from '@/composables/useSettingsSubTab'
import SettingsMfaCard from '@/components/settings/SettingsMfaCard.vue'
import SettingsOidcCard from '@/components/settings/SettingsOidcCard.vue'
import SettingsPasskeysCard from '@/components/settings/SettingsPasskeysCard.vue'
import SettingsPasswordCard from '@/components/settings/SettingsPasswordCard.vue'
import SettingsProxyCard from '@/components/settings/SettingsProxyCard.vue'
import SettingsSessions from '@/components/settings/SettingsSessions.vue'

const settings = defineModel<Settings>({ required: true })
/** Owned by the settings view, which keeps it in the address (RD-180-15). */
const activeTab = defineModel<string>('subTab', { default: 'signin' })
const { t } = useI18n()
const tabItems = computed(() => subTabItems('security', t))
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.security.eyebrow')"
        :title="t('settings.headers.security.title')"
        :description="t('settings.headers.security.description')"
        level="page"
      />
    </header>
    <UTabs
      v-model="activeTab"
      :items="tabItems"
      :unmount-on-hide="false"
      variant="pill"
      class="w-full"
      :ui="{ content: 'pt-4' }"
    >
      <template #signin>
        <div class="space-y-6">
          <SettingsPasswordCard />
          <div class="grid items-start gap-6 lg:grid-cols-2">
            <SettingsMfaCard />
            <SettingsPasskeysCard />
          </div>
          <SettingsOidcCard />
        </div>
      </template>
      <template #sessions>
        <div>
          <UCard as="section" data-settings-anchor="security.session_limits">
            <SectionHeader
              :eyebrow="t('system.session_limits.eyebrow')"
              :title="t('system.session_limits.title')"
              :description="t('system.session_limits.description')"
            />
            <div class="mt-4 grid gap-4">
              <UFormField :label="t('system.session_limits.idle_label')" :description="t('system.session_limits.idle_description')">
                <UInput v-model.number="settings.session_idle_hours" type="number" min="1" max="720" icon="i-lucide-timer" class="w-full">
                  <template #trailing><span class="font-mono text-xs text-muted">h</span></template>
                </UInput>
              </UFormField>
              <UFormField :label="t('system.session_limits.max_label')" :description="t('system.session_limits.max_description')">
                <UInput v-model.number="settings.session_max_hours" type="number" min="1" max="2160" icon="i-lucide-hourglass" class="w-full">
                  <template #trailing><span class="font-mono text-xs text-muted">h</span></template>
                </UInput>
              </UFormField>
            </div>
          </UCard>
          <SettingsSessions />
        </div>
      </template>
      <template #proxy>
        <SettingsProxyCard :model-value="settings" />
      </template>
    </UTabs>
  </div>
</template>
