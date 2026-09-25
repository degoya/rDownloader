<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'

const { t } = useI18n()
// The line the transfer rail carries, for the two screens that never see the rail: the login
// and the setup wizard. Health is a public route, so it answers before anybody is signed in.
const serviceVersion = ref('')

onMounted(async () => {
  const response = await api.GET('/api/v1/health')
  const data = response.data as { version?: string } | undefined
  if (data?.version) serviceVersion.value = data.version
})
</script>

<template>
  <div class="flex flex-wrap items-center justify-center gap-x-1.5 gap-y-1 border-t border-muted px-7 py-4 text-xs text-toned sm:px-9">
    <span class="font-semibold text-highlighted">rDownloader</span>
    <span v-if="serviceVersion" class="font-mono text-muted">v{{ serviceVersion }}</span>
    <span class="flex items-center gap-1">
      {{ t('common.footer.made_with') }}
      <UIcon name="i-lucide-heart" class="size-3 text-primary" />
      {{ t('common.footer.by', { author: 'Alexander Herling' }) }}
    </span>
  </div>
</template>
