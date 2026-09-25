<script setup lang="ts">
/**
 * API & MCP as a settings page: the same header every page carries, over the token card the
 * setup wizard also embeds. The card keeps its own card-level heading and the sentence that
 * names the endpoint, because the wizard shows it without this page around it.
 */
import { useI18n } from 'vue-i18n'

import SectionHeader from '@/components/SectionHeader.vue'
import SettingsMcpAccess from '@/components/settings/SettingsMcpAccess.vue'

const { t } = useI18n()
</script>

<template>
  <div>
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.mcp.eyebrow')"
        :title="t('settings.headers.mcp.title')"
        :description="t('settings.headers.mcp.description')"
        level="page"
      />
    </header>
    <SettingsMcpAccess />
  </div>
</template>
