<script setup lang="ts">
/**
 * Security: the administrator password, the second factor and the passkeys side by side on
 * large screens, how long a sign-in lasts, the sessions, and the reverse proxy this service
 * may believe. The page had no header until RD-110-29, no way to change the password until
 * RD-120-22, and a fixed twelve-hour sign-in until RD-130-09.
 */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsMfaCard from '@/components/settings/SettingsMfaCard.vue'
import SettingsPasskeysCard from '@/components/settings/SettingsPasskeysCard.vue'
import SettingsPasswordCard from '@/components/settings/SettingsPasswordCard.vue'
import SettingsProxyCard from '@/components/settings/SettingsProxyCard.vue'
import SettingsSessions from '@/components/settings/SettingsSessions.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
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
    <SettingsPasswordCard />
    <div class="grid items-start gap-6 lg:grid-cols-2">
      <SettingsMfaCard />
      <SettingsPasskeysCard />
    </div>
    <section class="border border-muted bg-default p-5">
      <SectionHeader
        :eyebrow="t('system.session_limits.eyebrow')"
        :title="t('system.session_limits.title')"
        :description="t('system.session_limits.description')"
      />
      <div class="mt-4 grid gap-4 md:grid-cols-2">
        <UFormField :label="t('system.session_limits.idle_label')" :description="t('system.session_limits.idle_description')">
          <UInput v-model.number="settings.session_idle_hours" type="number" min="1" max="720" icon="i-lucide-timer" class="mt-2 w-full">
            <template #trailing><span class="font-mono text-xs text-muted">h</span></template>
          </UInput>
        </UFormField>
        <UFormField :label="t('system.session_limits.max_label')" :description="t('system.session_limits.max_description')">
          <UInput v-model.number="settings.session_max_hours" type="number" min="1" max="2160" icon="i-lucide-hourglass" class="mt-2 w-full">
            <template #trailing><span class="font-mono text-xs text-muted">h</span></template>
          </UInput>
        </UFormField>
      </div>
    </section>
    <SettingsSessions />
    <SettingsProxyCard :model-value="settings" />
  </div>
</template>
