<script setup lang="ts">
import { useI18n } from 'vue-i18n'

const { t } = useI18n()
</script>

<template>
  <div class="signal-grid pointer-events-none fixed inset-0 z-50 grid place-items-center border-4 border-dashed border-primary bg-default/90 text-center backdrop-blur-sm">
    <div>
      <UIcon name="i-lucide-file-up" class="mx-auto mb-4 size-12 text-primary" />
      <p class="text-lg font-semibold text-highlighted">{{ t('linkgrabber.nzb.drop.title') }}</p>
      <p class="mt-1 text-sm text-muted">{{ t('linkgrabber.nzb.drop.hint') }}</p>
    </div>
  </div>
</template>
