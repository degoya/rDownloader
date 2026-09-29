<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { BundledCategory, BundledService } from '@/api/bundledPlugins'

/**
 * The bundle by service, filtered by category and searched by name (RD-160-05).
 *
 * Two uses, one list: the wizard's "Your services" step picks with a checkbox per service and
 * installs on "Continue" (`mode="select"`), the plugin manager installs one service per click
 * (`mode="install"`). A service that is installed is shown ticked and cannot be unticked here —
 * removing a plugin asks first and can be refused while a download uses it, which is the plugin
 * manager's job, not a checkbox's.
 */
const props = defineProps<{
  services: BundledService[]
  mode: 'select' | 'install'
  /** The key whose install is running, or `true` while a whole run is. */
  busy?: string | boolean
}>()
const emit = defineEmits<{ install: [key: string] }>()
const selected = defineModel<string[]>('selected', { default: () => [] })

const { t } = useI18n()
const ALL = '__all__'
const category = ref<string>(ALL)
const query = ref('')

const categories = computed(() => {
  const counts = new Map<BundledCategory, number>()
  for (const service of props.services) counts.set(service.category, (counts.get(service.category) ?? 0) + 1)
  return [
    { value: ALL, label: t('plugins.bundled.all'), count: props.services.length },
    // In the server's order, which is the order the categories are meant to be read in.
    ...[...counts.entries()].map(([value, count]) => ({ value, label: t(`plugins.bundled.category.${value}`), count }))
  ]
})

const visible = computed(() => {
  const needle = query.value.trim().toLocaleLowerCase()
  return props.services.filter(service =>
    (category.value === ALL || service.category === category.value)
    && (!needle
      || service.name.toLocaleLowerCase().includes(needle)
      || service.key.includes(needle)
      || service.plugins.some(plugin => plugin.name.toLocaleLowerCase().includes(needle))))
})

/** The visible services under their category heading, in the server's order. */
const groups = computed(() => {
  const byCategory = new Map<BundledCategory, BundledService[]>()
  for (const service of visible.value) {
    const list = byCategory.get(service.category) ?? []
    list.push(service)
    byCategory.set(service.category, list)
  }
  return [...byCategory.entries()].map(([value, services]) => ({ value, services }))
})

const isInstalled = (service: BundledService): boolean => service.state === 'installed'

function toggle(service: BundledService, on: boolean | 'indeterminate'): void {
  const rest = selected.value.filter(key => key !== service.key)
  selected.value = on === true ? [...rest, service.key] : rest
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex flex-col gap-3 sm:flex-row sm:items-center">
      <UInput
        v-model="query"
        class="sm:w-72"
        icon="i-lucide-search"
        :placeholder="t('plugins.bundled.search')"
        :aria-label="t('plugins.bundled.search')"
        data-testid="bundled-search"
      />
      <div class="flex flex-wrap items-center gap-2" role="group" :aria-label="t('plugins.bundled.filter_label')">
        <UButton
          v-for="entry in categories"
          :key="entry.value"
          class="max-w-full"
          size="xs"
          :color="category === entry.value ? 'primary' : 'neutral'"
          :variant="category === entry.value ? 'solid' : 'outline'"
          :aria-pressed="category === entry.value"
          @click="category = entry.value"
        >
          <span class="whitespace-normal text-left">{{ entry.label }}</span>
          <UBadge size="xs" color="neutral" variant="subtle" class="font-mono">{{ entry.count }}</UBadge>
        </UButton>
      </div>
    </div>

    <p v-if="!visible.length" class="border border-dashed border-muted p-5 text-center text-sm text-muted">
      {{ t('plugins.bundled.no_match', { query: query.trim() }) }}
    </p>

    <section v-for="group in groups" :key="group.value" class="space-y-2">
      <p class="eyebrow">{{ t(`plugins.bundled.category.${group.value}`) }}</p>
      <div class="grid gap-2 md:grid-cols-2">
        <div
          v-for="service in group.services"
          :key="service.key"
          class="flex items-start justify-between gap-3 border border-muted p-3"
          :data-testid="`bundled-service-${service.key}`"
        >
          <UCheckbox
            v-if="mode === 'select'"
            class="min-w-0"
            :model-value="isInstalled(service) || selected.includes(service.key)"
            :disabled="isInstalled(service) || !!busy"
            :label="service.name"
            :description="service.description"
            @update:model-value="(value: boolean | 'indeterminate') => toggle(service, value)"
          />
          <div v-else class="min-w-0">
            <p class="font-medium text-highlighted">{{ service.name }}</p>
            <p class="mt-1 text-sm leading-5 text-muted">{{ service.description }}</p>
          </div>
          <div class="flex shrink-0 flex-col items-end gap-2">
            <UBadge v-if="isInstalled(service)" color="success" variant="subtle" size="sm">
              {{ t('plugins.bundled.state.installed') }}
            </UBadge>
            <UBadge v-else-if="service.state === 'partial'" color="warning" variant="subtle" size="sm">
              {{ t('plugins.bundled.state.partial') }}
            </UBadge>
            <UBadge v-if="service.needs_account && !isInstalled(service)" color="neutral" variant="outline" size="sm">
              {{ t('plugins.bundled.needs_account') }}
            </UBadge>
            <UButton
              v-if="mode === 'install' && !isInstalled(service)"
              size="xs"
              icon="i-lucide-package-plus"
              :label="t('plugins.bundled.install')"
              :loading="busy === service.key"
              :disabled="!!busy && busy !== service.key"
              @click="emit('install', service.key)"
            />
          </div>
        </div>
      </div>
    </section>
  </div>
</template>
