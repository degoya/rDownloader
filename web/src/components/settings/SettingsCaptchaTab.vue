<script setup lang="ts">
/**
 * Captcha & solver: the card that used to sit under Network (RD-110-29). Its configuration is a
 * separate, redacted API document, saved by the page-level save button through `saveCaptcha`,
 * the way the network page used to forward it.
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import SectionHeader from '@/components/SectionHeader.vue'
import SettingsCaptchaCard from '@/components/SettingsCaptchaCard.vue'

const emit = defineEmits<{ error: [string] }>()
const { t } = useI18n()
const captchaCard = ref<InstanceType<typeof SettingsCaptchaCard> | null>(null)

async function saveCaptcha(): Promise<boolean> {
  return await captchaCard.value?.save() ?? true
}

/** The card holds its own form, which the page loses when it leaves this section (RD-180-16). */
const dirty = computed(() => captchaCard.value?.dirty ?? false)

defineExpose({ saveCaptcha, dirty })
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.captcha.eyebrow')"
        :title="t('settings.headers.captcha.title')"
        :description="t('settings.headers.captcha.description')"
        level="page"
      />
    </header>
    <SettingsCaptchaCard ref="captchaCard" @error="(text: string) => emit('error', text)" />
  </div>
</template>
