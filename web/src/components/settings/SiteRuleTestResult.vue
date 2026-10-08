<script setup lang="ts">
/**
 * What a trial run of a site rule found (RD-110-08): the links with this installation's verdict
 * on each, the package name and what was dropped. A rule with groups (RD-1170-02) shows one
 * block per package instead of one list, each link with its mirror set — the packages and
 * mirrors the LinkGrabber will show for the same page.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SiteRuleTestResult } from '@/api/types'

const props = defineProps<{ result: SiteRuleTestResult }>()

const { t } = useI18n()

/** Groups only exist for a rule that declares them; every other answer is one list. */
const groups = computed(() => props.result.groups ?? [])
/** A two-stage rule (RD-1170-03) lists entries to choose from and resolves none on a trial. */
const entries = computed(() => props.result.entries ?? [])

/** An entry's attributes as one line: `season 1 · episode 7 · 720p`. */
function attributeLine(attributes: Record<string, string>): string {
  return Object.entries(attributes).map(([name, value]) => `${name} ${value}`).join(' · ')
}
const verdicts = computed(() => new Map(props.result.links.map(link => [link.url, link])))

function verdictColor(verdict: string): 'success' | 'error' | 'neutral' {
  if (verdict === 'confirmed' || verdict === 'claimed') return 'success'
  if (verdict === 'not-a-file') return 'error'
  return 'neutral'
}

function verdictTitle(url: string): string {
  const link = verdicts.value.get(url)
  if (!link) return ''
  return link.code ? t(`server.codes.${link.code}`) : t(`siterules.test.verdicts.${link.verdict}`)
}
</script>

<template>
  <div class="mt-3 border border-muted p-3">
    <p class="truncate font-mono text-xs text-muted">{{ t('siterules.test.crawled', { address: props.result.address }) }}</p>
    <p v-if="props.result.error" role="alert" class="mt-2 text-sm text-error">
      {{ t(`server.codes.${props.result.error}`) }}
    </p>
    <template v-else>
      <div class="mt-2 flex flex-wrap items-center gap-2 text-xs">
        <UBadge color="neutral" variant="outline">{{ t('siterules.test.pages', props.result.pages_fetched) }}</UBadge>
        <UBadge color="success" variant="subtle">{{ t('siterules.test.kept', { count: props.result.kept }) }}</UBadge>
        <UBadge v-if="props.result.refused" color="error" variant="subtle">{{ t('siterules.test.refused', { count: props.result.refused }) }}</UBadge>
        <UBadge v-if="props.result.mirrors" color="neutral" variant="subtle" :title="t('siterules.test.mirrors')">{{ t('siterules.test.mirrors') }}</UBadge>
        <UBadge v-if="groups.length" color="neutral" variant="subtle">{{ t('siterules.test.groups', groups.length) }}</UBadge>
        <UBadge v-if="entries.length" color="neutral" variant="subtle">{{ t('siterules.test.entries', entries.length) }}</UBadge>
      </div>
      <p class="mt-2 text-xs text-muted">
        {{ props.result.package_name
          ? `${t('siterules.test.package')}: ${props.result.package_name}`
          : t('siterules.test.package_none') }}
      </p>
      <template v-if="entries.length">
        <p class="mt-2 text-xs text-muted">{{ t('siterules.test.entries_hint') }}</p>
        <ul class="mt-1 space-y-1" :aria-label="t('siterules.test.entries', entries.length)">
          <li v-for="(entry, index) in entries" :key="index" class="flex flex-wrap items-center gap-2">
            <span class="min-w-0 flex-1 truncate font-mono text-2xs">{{ entry.label ?? t('siterules.test.group_unnamed') }}</span>
            <span class="text-2xs text-muted">{{ attributeLine(entry.attributes) }}</span>
          </li>
        </ul>
      </template>
      <p v-else-if="!props.result.links.length" class="mt-2 text-xs text-muted">{{ t('siterules.test.no_links') }}</p>
      <ul v-else-if="!groups.length" class="mt-2 space-y-1">
        <li v-for="link in props.result.links" :key="link.url" class="flex flex-wrap items-center gap-2">
          <span class="min-w-0 flex-1 truncate font-mono text-2xs">{{ link.url }}</span>
          <UBadge :color="verdictColor(link.verdict)" variant="subtle" :title="verdictTitle(link.url)">
            {{ t(`siterules.test.verdicts.${link.verdict}`) }}
          </UBadge>
        </li>
      </ul>
      <template v-else>
        <section
          v-for="(group, index) in groups"
          :key="index"
          class="mt-3"
          :aria-label="group.name ?? t('siterules.test.group_unnamed')"
        >
          <USeparator class="mb-2" />
          <div class="flex flex-wrap items-center gap-2">
            <span class="min-w-0 flex-1 truncate text-xs font-medium text-highlighted">
              {{ group.name ?? t('siterules.test.group_unnamed') }}
            </span>
            <UBadge color="neutral" variant="outline">{{ t('siterules.test.group_links', group.links.length) }}</UBadge>
          </div>
          <ul class="mt-1 space-y-1">
            <li v-for="link in group.links" :key="link.url" class="flex flex-wrap items-center gap-2">
              <span class="min-w-0 flex-1 truncate font-mono text-2xs">{{ link.url }}</span>
              <UBadge v-if="link.mirror" color="neutral" variant="subtle" :title="t('siterules.test.mirror_hint')">
                {{ t('siterules.test.mirror', { set: link.mirror }) }}
              </UBadge>
              <UBadge
                v-if="verdicts.get(link.url)"
                :color="verdictColor(verdicts.get(link.url)?.verdict ?? '')"
                variant="subtle"
                :title="verdictTitle(link.url)"
              >
                {{ t(`siterules.test.verdicts.${verdicts.get(link.url)?.verdict}`) }}
              </UBadge>
            </li>
          </ul>
        </section>
      </template>
    </template>
  </div>
</template>
