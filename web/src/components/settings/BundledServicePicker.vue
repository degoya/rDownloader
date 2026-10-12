<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { BundledCategory, BundledPlugin, BundledService, PluginOrigin } from '@/api/bundledPlugins'
import { permissionLabels } from './pluginPermissions'

/**
 * The services by category, filtered and searched by name (RD-160-05).
 *
 * Two uses, one list: the wizard's "Your services" step picks with a checkbox per service
 * (`mode="select"`), the plugin manager's catalogue shows every service with what it can do now
 * (`mode="catalogue"`, RD-1260-02). In the wizard a checkbox is the whole choice: the step ticks
 * what is installed, and unticking one removes it on "Continue" after a confirmation (RD-180-14).
 * The catalogue installs a bundled service on a click, opens the preview for what only a
 * repository offers (`previewOnly`) and for a waiting update, and names each service's origin;
 * removing there is the installed plugin's own delete, one version at a time.
 */
const props = defineProps<{
  services: BundledService[]
  mode: 'select' | 'catalogue'
  /** The key whose install is running, or `true` while a whole run is. */
  busy?: string | boolean
  /** Keys of the services that install through the preview: what only a repository offers. */
  previewOnly?: string[]
}>()
const emit = defineEmits<{
  install: [key: string]
  /** Review a package before it installs: the update of `plugin`, or `plugin` itself. */
  review: [plugin: BundledPlugin]
}>()
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
const viaPreview = (service: BundledService): boolean => props.previewOnly?.includes(service.key) === true

/** Where the service's first plugin came from when installed, or would come from now. */
function origin(service: BundledService): PluginOrigin | null {
  const primary = service.plugins[0]
  if (!primary) return null
  return (isInstalled(service) ? primary.installed_origin : null) ?? primary.origin ?? null
}

/** The first plugin of the service with an update waiting. */
const updateOf = (service: BundledService): BundledPlugin | undefined => service.plugins.find(plugin => plugin.update)

/** What a waiting update adds, for the mark's tooltip (RD-160-09). */
function addedHint(plugin: BundledPlugin): string {
  const added = plugin.update ? permissionLabels(t, plugin.update.added_permissions) : []
  const hint = t('plugins.updates.adds_permissions_hint')
  return added.length ? `${hint}\n${t('plugins.updates.added_permissions', { permissions: added.join(', ') })}` : hint
}

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
      <URadioGroup
        v-model="category"
        :items="categories"
        variant="card"
        indicator="hidden"
        orientation="horizontal"
        size="xs"
        :aria-label="t('plugins.bundled.filter_label')"
      >
        <template #label="{ item }">
          {{ item.label }}<UBadge size="xs" color="neutral" variant="subtle" class="ms-1.5 font-mono">{{ item.count }}</UBadge>
        </template>
      </URadioGroup>
    </div>

    <UEmpty v-if="!visible.length" :description="t('plugins.bundled.no_match', { query: query.trim() })" />

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
            :model-value="selected.includes(service.key)"
            :disabled="!!busy"
            :label="service.name"
            :description="service.description"
            @update:model-value="(value: boolean | 'indeterminate') => toggle(service, value)"
          />
          <div v-else class="min-w-0">
            <p class="font-medium text-highlighted">{{ service.name }}</p>
            <p v-if="service.description" class="mt-1 text-sm leading-5 text-muted">{{ service.description }}</p>
            <p v-if="updateOf(service)?.update" class="mt-1 font-mono text-xs text-muted" data-update-versions>
              {{ t('plugins.updates.version_change', { from: updateOf(service)?.update?.installed_version, to: updateOf(service)?.update?.offer.package.version }) }}
            </p>
          </div>
          <div class="flex shrink-0 flex-col items-end gap-2">
            <UBadge v-if="isInstalled(service)" color="success" variant="subtle" size="sm">
              {{ t('plugins.bundled.state.installed') }}
            </UBadge>
            <UBadge v-else-if="service.state === 'partial'" color="warning" variant="subtle" size="sm">
              {{ t('plugins.bundled.state.partial') }}
            </UBadge>
            <template v-if="mode === 'catalogue'">
              <UBadge v-if="service.update_available" color="primary" variant="subtle" size="sm" data-update-badge>
                {{ t('plugins.catalogue.state.update') }}
              </UBadge>
              <UBadge
                v-if="origin(service)"
                color="neutral"
                variant="outline"
                size="sm"
                :title="t('plugins.catalogue.origin_hint')"
                :data-origin="origin(service)"
              >
                {{ t(`plugins.catalogue.origin.${origin(service)}`) }}
              </UBadge>
              <UBadge v-if="updateOf(service)?.update?.policy === 'automatic'" color="warning" variant="subtle" size="sm" :title="t('plugins.updates.automatic_hint')">
                {{ t('plugins.updates.automatic') }}
              </UBadge>
              <UBadge
                v-if="updateOf(service)?.update?.adds_permissions"
                color="error"
                variant="subtle"
                size="sm"
                data-adds-permissions
                :title="addedHint(updateOf(service) as BundledPlugin)"
              >
                {{ t('plugins.updates.adds_permissions') }}
              </UBadge>
            </template>
            <UBadge v-if="service.needs_account && !isInstalled(service)" color="neutral" variant="outline" size="sm">
              {{ t('plugins.bundled.needs_account') }}
            </UBadge>
            <template v-if="mode === 'catalogue'">
              <UButton
                v-if="updateOf(service)"
                size="xs"
                color="primary"
                variant="outline"
                icon="i-lucide-scan-search"
                :label="t('plugins.updates.review')"
                :disabled="!!busy"
                @click="emit('review', updateOf(service) as BundledPlugin)"
              />
              <UButton
                v-else-if="!isInstalled(service) && viaPreview(service) && service.plugins[0]"
                size="xs"
                color="neutral"
                variant="outline"
                icon="i-lucide-scan-search"
                :label="t('plugins.updates.review_package')"
                :disabled="!!busy"
                @click="emit('review', service.plugins[0])"
              />
              <UButton
                v-else-if="!isInstalled(service) && !viaPreview(service)"
                size="xs"
                icon="i-lucide-package-plus"
                :label="t('plugins.bundled.install')"
                :loading="busy === service.key"
                :disabled="!!busy && busy !== service.key"
                @click="emit('install', service.key)"
              />
            </template>
          </div>
        </div>
      </div>
    </section>
  </div>
</template>
