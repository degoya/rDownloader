<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Account, Category, DownloadPriority, ProxyProfile } from '@/api/types'
import { priorityItems } from '@/utils/format'
import { NO_SELECTION } from '@/utils/select'

const PRIORITY_ITEMS = computed(() => priorityItems())
const { t } = useI18n()

const props = defineProps<{ categories: Category[], accounts: Account[], proxies: ProxyProfile[], busy: boolean }>()
const emit = defineEmits<{
  submit: [payload: { url: string, categoryId?: string, accountId?: string, proxyProfileId?: string, priority: DownloadPriority }]
}>()

const url = ref('')
const categoryId = ref(NO_SELECTION)
const accountId = ref(NO_SELECTION)
const proxyProfileId = ref(NO_SELECTION)
const priority = ref<DownloadPriority>('normal')

const categoryItems = computed(() => [
  { label: t('downloads.add.default_category'), value: NO_SELECTION },
  ...props.categories.map(category => ({ label: category.name, value: category.id }))
])
const accountItems = computed(() => [
  { label: t('downloads.add.auto_resolver'), value: NO_SELECTION },
  ...props.accounts.filter(account => account.enabled).map(account => ({ label: `${account.label} · ${account.provider}`, value: account.id }))
])
const proxyItems = computed(() => [
  { label: t('downloads.add.proxy_default'), value: NO_SELECTION },
  ...props.proxies.map(proxy => ({ label: `${proxy.name} · ${proxy.kind}`, value: proxy.id }))
])

function submit(): void {
  if (!url.value) return
  emit('submit', {
    url: url.value,
    ...(categoryId.value !== NO_SELECTION ? { categoryId: categoryId.value } : {}),
    ...(accountId.value !== NO_SELECTION ? { accountId: accountId.value } : {}),
    ...(proxyProfileId.value !== NO_SELECTION ? { proxyProfileId: proxyProfileId.value } : {}),
    priority: priority.value
  })
}

defineExpose({ reset: () => { url.value = '' } })
</script>

<template>
  <section class="border border-muted bg-elevated p-4">
    <p class="eyebrow mb-3">{{ t('downloads.add.eyebrow') }}</p>
    <form class="grid gap-2 lg:grid-cols-[minmax(280px,1fr)_repeat(4,minmax(130px,0.4fr))_auto]" @submit.prevent="submit">
      <UInput v-model="url" type="url" required icon="i-lucide-link" :placeholder="t('downloads.add.url_placeholder')" size="lg" />
      <USelect v-model="categoryId" :items="categoryItems" size="lg" :aria-label="t('downloads.add.category_aria')" />
      <USelect v-model="accountId" :items="accountItems" size="lg" :aria-label="t('downloads.add.account_aria')" />
      <USelect v-model="proxyProfileId" :items="proxyItems" size="lg" :aria-label="t('downloads.add.proxy_aria')" />
      <USelect v-model="priority" :items="PRIORITY_ITEMS" value-key="value" size="lg" :aria-label="t('downloads.add.priority_aria')" />
      <UButton type="submit" icon="i-lucide-plus" :label="t('downloads.add.submit')" size="lg" :loading="props.busy" />
    </form>
  </section>
</template>
