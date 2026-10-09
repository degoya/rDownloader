<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Account, Category, DownloadPriority, ProxyProfile } from '@/api/types'
import SearchableSelect from '@/components/SearchableSelect.vue'
import { priorityItems } from '@/utils/format'
import { NO_SELECTION } from '@/utils/select'

const PRIORITY_ITEMS = computed(() => priorityItems())
const { t } = useI18n()

export interface DirectAddPayload { url: string, categoryId?: string, accountId?: string, proxyProfileId?: string, priority: DownloadPriority }

const props = defineProps<{ categories: Category[], accounts: Account[], proxies: ProxyProfile[] }>()
const emit = defineEmits<{ submit: [payload: DirectAddPayload] }>()

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

</script>

<template>
  <!--
    The fields of the direct job; `DirectAddModal` around it carries the title, the feedback and
    the submit in its footer (`form="direct-add-form"`), so Enter in any field still submits
    (RD-1220-03). The address takes the focus when the dialog opens.
  -->
  <form id="direct-add-form" class="space-y-3" @submit.prevent="submit">
    <UFormField :label="t('downloads.add.url_label')" required>
      <UInput v-model="url" type="url" required autofocus icon="i-lucide-link" :placeholder="t('downloads.add.url_placeholder')" class="w-full" data-testid="direct-add-url" />
    </UFormField>
    <UFormField :label="t('downloads.add.category_aria')">
      <SearchableSelect v-model="categoryId" :items="categoryItems" class="w-full" :aria-label="t('downloads.add.category_aria')" />
    </UFormField>
    <UFormField :label="t('downloads.add.account_aria')">
      <SearchableSelect v-model="accountId" :items="accountItems" class="w-full" :aria-label="t('downloads.add.account_aria')" />
    </UFormField>
    <UFormField :label="t('downloads.add.proxy_aria')">
      <SearchableSelect v-model="proxyProfileId" :items="proxyItems" class="w-full" :aria-label="t('downloads.add.proxy_aria')" />
    </UFormField>
    <UFormField :label="t('downloads.add.priority_aria')">
      <USelect v-model="priority" :items="PRIORITY_ITEMS" value-key="value" class="w-full" :aria-label="t('downloads.add.priority_aria')" />
    </UFormField>
  </form>
</template>
