<script setup lang="ts">
/**
 * The About page (RD-130-12): which build is running, where it comes from, who made it, and
 * under which licenses the parts it ships are.
 *
 * Two reads, because they answer at different sizes: the head is a few lines, the dependency
 * list about a thousand rows, and the MCP tool that reads the head has no use for the rows.
 * Every figure comes from the service — the commit and build time from the binary's build
 * script, the dependency list from the generated file — so this page keeps nothing of its own.
 *
 * An address that does not lead anywhere yet is written out and marked "not yet published"
 * instead of being drawn as a link, and one that is not decided yet is the mark alone
 * (`design.md`, "An Address That Leads Nowhere Yet").
 */
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { About, AboutLink, ThirdPartyLicenses, ThirdPartyPackage } from '@/api/types'
import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useFetchState } from '@/composables/useFetchState'

type Ecosystem = 'rust' | 'npm'

const { t } = useI18n()
const about = ref<About | null>(null)
const licenses = ref<ThirdPartyLicenses | null>(null)
const { loading, loadError, load: trackAbout } = useFetchState()
const { loading: licensesLoading, loadError: licensesError, load: trackLicenses } = useFetchState()
const open = reactive<Record<Ecosystem, boolean>>({ rust: false, npm: false })
const filter = ref('')

onMounted(() => {
  void trackAbout(async () => {
    const response = await api.GET('/api/v1/system/about')
    if (!response.data) return responseError(response)
    about.value = response.data
    return null
  })
  void trackLicenses(async () => {
    const response = await api.GET('/api/v1/system/about/licenses')
    if (!response.data) return responseError(response)
    licenses.value = response.data
    return null
  })
})

const facts = computed(() => {
  const current = about.value
  if (!current) return []
  const unknown = t('settings.about.build.unknown')
  return [
    { key: 'version', label: t('settings.about.build.version'), value: current.version },
    { key: 'commit', label: t('settings.about.build.commit'), value: current.commit ?? unknown },
    { key: 'built', label: t('settings.about.build.built'), value: current.built ?? unknown },
    { key: 'plugin_contract', label: t('settings.about.build.plugin_contract'), value: current.plugin_contracts.join(', ') },
    { key: 'platform', label: t('settings.about.build.platform'), value: current.platform }
  ]
})

/** Typed against the schema's enum, so a new kind of address cannot arrive without a label. */
const LINK_LABELS: Record<AboutLink['kind'], string> = {
  source: 'settings.about.links.source',
  website: 'settings.about.links.website',
  handbook: 'settings.about.links.handbook',
  security: 'settings.about.links.security',
  changelog: 'settings.about.links.changelog'
}

const ECOSYSTEMS: readonly { value: Ecosystem, labelKey: string }[] = [
  { value: 'rust', labelKey: 'settings.about.licenses.rust' },
  { value: 'npm', labelKey: 'settings.about.licenses.npm' }
]

function packages(ecosystem: Ecosystem): ThirdPartyPackage[] {
  return licenses.value?.[ecosystem] ?? []
}

/** How many packages carry each license, most common first: the list's shape at a glance. */
function summary(ecosystem: Ecosystem): { license: string, count: number }[] {
  const counts = new Map<string, number>()
  for (const entry of packages(ecosystem)) {
    counts.set(entry.license, (counts.get(entry.license) ?? 0) + 1)
  }
  return [...counts.entries()]
    .map(([license, count]) => ({ license, count }))
    .sort((left, right) => right.count - left.count || left.license.localeCompare(right.license))
}

function visible(ecosystem: Ecosystem): ThirdPartyPackage[] {
  const term = filter.value.trim().toLowerCase()
  if (!term) return packages(ecosystem)
  return packages(ecosystem).filter(entry =>
    entry.name.toLowerCase().includes(term) || entry.license.toLowerCase().includes(term))
}

const anyOpen = computed(() => open.rust || open.npm)
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.about.eyebrow')"
        :title="t('settings.headers.about.title')"
        :description="t('settings.headers.about.description')"
        level="page"
      />
    </header>

    <DataState :loading="loading" :error="loadError" :rows="5" />

    <template v-if="about">
      <section class="border border-muted bg-default p-5" data-testid="about-build">
        <SectionHeader :eyebrow="t('settings.about.build.eyebrow')" :title="t('settings.about.build.title')" />
        <dl class="mt-4 grid gap-x-6 gap-y-3 sm:grid-cols-[max-content_1fr]">
          <template v-for="fact in facts" :key="fact.key">
            <dt class="eyebrow">{{ fact.label }}</dt>
            <dd class="numeric break-all text-sm text-highlighted" :data-fact="fact.key">{{ fact.value }}</dd>
          </template>
        </dl>
      </section>

      <section class="border border-muted bg-default p-5" data-testid="about-links">
        <SectionHeader :eyebrow="t('settings.about.links.eyebrow')" :title="t('settings.about.links.title')" />
        <ul class="mt-4 space-y-2">
          <li
            v-for="link in about.links"
            :key="link.kind"
            class="flex flex-wrap items-baseline gap-x-3 gap-y-1 text-sm"
            :data-link="link.kind"
          >
            <span class="min-w-48 text-toned">{{ t(LINK_LABELS[link.kind]) }}</span>
            <a
              v-if="link.published && link.url"
              :href="link.url"
              target="_blank"
              rel="noopener noreferrer"
              class="break-all font-mono text-xs text-primary underline-offset-2 hover:underline"
            >{{ link.url }}</a>
            <template v-else>
              <span v-if="link.url" class="break-all font-mono text-xs text-muted">{{ link.url }}</span>
              <UBadge color="neutral" variant="subtle" size="sm" data-unpublished>{{ t('settings.about.links.unpublished') }}</UBadge>
            </template>
          </li>
        </ul>
      </section>

      <section class="border border-muted bg-default p-5" data-testid="about-credits">
        <SectionHeader :eyebrow="t('settings.about.credits.eyebrow')" :title="t('settings.about.credits.title')" />
        <dl class="mt-4 grid gap-x-6 gap-y-3 sm:grid-cols-[max-content_1fr]">
          <dt class="eyebrow">{{ t('settings.about.credits.author') }}</dt>
          <dd class="text-sm text-highlighted">{{ about.authors.join(', ') }}</dd>
        </dl>
        <p class="mt-3 max-w-3xl text-sm leading-6 text-muted">{{ t('settings.about.credits.thanks') }}</p>
      </section>

      <section class="border border-muted bg-default p-5" data-testid="about-licenses">
        <SectionHeader :eyebrow="t('settings.about.licenses.eyebrow')" :title="t('settings.about.licenses.title')" />

        <h3 class="mt-5 text-sm font-semibold text-highlighted">{{ t('settings.about.licenses.own') }}</h3>
        <p class="mt-1 text-sm text-toned">{{ t('settings.about.licenses.own_text', { license: about.license }) }}</p>

        <h3 class="mt-5 text-sm font-semibold text-highlighted">{{ t('settings.about.licenses.tools') }}</h3>
        <p class="mt-1 text-xs text-muted">{{ t('settings.about.licenses.tools_hint') }}</p>
        <div class="mt-3 overflow-x-auto">
          <table class="w-full text-left text-sm" data-testid="about-tools">
            <thead class="eyebrow">
              <tr>
                <th class="py-1 pr-4 font-normal">{{ t('settings.about.licenses.column_name') }}</th>
                <th class="py-1 pr-4 font-normal">{{ t('settings.about.licenses.column_license') }}</th>
                <th class="py-1 font-normal">{{ t('settings.about.licenses.column_file') }}</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="tool in about.bundled_tools" :key="tool.name" class="border-t border-muted">
                <td class="py-1.5 pr-4">
                  <a :href="tool.homepage" target="_blank" rel="noopener noreferrer" class="text-primary underline-offset-2 hover:underline">{{ tool.name }}</a>
                </td>
                <td class="py-1.5 pr-4 font-mono text-xs">{{ tool.license }}</td>
                <td class="py-1.5 font-mono text-xs text-muted">{{ tool.file }}</td>
              </tr>
            </tbody>
          </table>
        </div>

        <h3 class="mt-5 text-sm font-semibold text-highlighted">{{ t('settings.about.licenses.dependencies') }}</h3>
        <p class="mt-1 text-xs text-muted">{{ t('settings.about.licenses.dependencies_hint') }}</p>
        <DataState class="mt-3" :loading="licensesLoading" :error="licensesError" variant="inline" />
        <template v-if="licenses">
          <UInput
            v-if="anyOpen"
            v-model="filter"
            icon="i-lucide-search"
            :placeholder="t('settings.about.licenses.filter')"
            :aria-label="t('settings.about.licenses.filter')"
            class="mt-3 w-full sm:max-w-sm"
            data-testid="about-filter"
          />
          <div v-for="{ value: ecosystem, labelKey } in ECOSYSTEMS" :key="ecosystem" class="mt-4" :data-ecosystem="ecosystem">
            <div class="flex flex-wrap items-center justify-between gap-2">
              <h4 class="text-sm text-highlighted">
                {{ t(labelKey) }}
                <span class="numeric text-muted">· {{ packages(ecosystem).length }}</span>
              </h4>
              <UButton
                v-if="packages(ecosystem).length"
                color="neutral"
                variant="ghost"
                size="sm"
                :icon="open[ecosystem] ? 'i-lucide-chevron-up' : 'i-lucide-chevron-down'"
                :label="open[ecosystem] ? t('settings.about.licenses.hide') : t('settings.about.licenses.show', { count: packages(ecosystem).length })"
                :aria-expanded="open[ecosystem]"
                :data-toggle="ecosystem"
                @click="open[ecosystem] = !open[ecosystem]"
              />
            </div>
            <ul class="mt-2 flex flex-wrap gap-1.5" :aria-label="t('settings.about.licenses.column_license')">
              <li v-for="entry in summary(ecosystem)" :key="entry.license">
                <UBadge color="neutral" variant="outline" size="sm" class="font-mono">{{ entry.license }} · {{ entry.count }}</UBadge>
              </li>
            </ul>
            <div v-if="open[ecosystem]" class="mt-3 max-h-96 overflow-auto border border-muted">
              <p v-if="!visible(ecosystem).length" class="p-3 text-sm text-muted">{{ t('settings.about.licenses.no_match') }}</p>
              <table v-else class="w-full text-left text-xs" :data-list="ecosystem">
                <thead class="eyebrow sticky top-0 bg-default">
                  <tr>
                    <th class="px-3 py-1 font-normal">{{ t('settings.about.licenses.column_name') }}</th>
                    <th class="px-3 py-1 font-normal">{{ t('settings.about.licenses.column_version') }}</th>
                    <th class="px-3 py-1 font-normal">{{ t('settings.about.licenses.column_license') }}</th>
                  </tr>
                </thead>
                <tbody class="font-mono">
                  <tr v-for="entry in visible(ecosystem)" :key="`${entry.name}@${entry.version}`" class="border-t border-muted">
                    <td class="px-3 py-1 break-all">{{ entry.name }}</td>
                    <td class="numeric px-3 py-1">{{ entry.version }}</td>
                    <td class="px-3 py-1">{{ entry.license }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </template>
      </section>
    </template>
  </div>
</template>
