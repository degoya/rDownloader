<script setup lang="ts">
/**
 * What gets written into the finished file, and what gets cut out of it (RD-080-03).
 *
 * Every piece is its own toggle rather than one "add metadata" switch, because embedding is
 * irreversible in practice — nobody re-downloads a file to strip a wrong tag. SponsorBlock
 * gets stricter treatment again: marking segments as chapters leaves the media alone, while
 * removing them re-cuts it against a crowd-sourced database that is occasionally wrong, so
 * the destructive mode says so before it is chosen.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { EmbedWarning, MediaEmbedPolicy, SponsorCategory, SponsorMode } from '@/api/types'

const { t } = useI18n()
const props = defineProps<{
  embed: MediaEmbedPolicy
  warnings?: EmbedWarning[]
  canTranscode: boolean
  busy?: boolean
}>()
const emit = defineEmits<{ change: [embed: MediaEmbedPolicy] }>()

const PIECES = ['thumbnail', 'chapters', 'metadata', 'info_json'] as const
const MODES: SponsorMode[] = ['off', 'mark', 'remove']
const CATEGORIES: SponsorCategory[] = [
  'sponsor', 'intro', 'outro', 'self_promo', 'preview', 'filler', 'interaction', 'music_offtopic'
]

const modeItems = computed(() => MODES.map(value => ({ label: t(`linkgrabber.media.embed.modes.${value}`), value })))
const categoryItems = computed(() => CATEGORIES.map(value => ({ label: t(`linkgrabber.media.embed.categories.${value}`), value })))
const sponsorOff = computed(() => props.embed.sponsorblock.mode === 'off')
const destructive = computed(() => props.embed.sponsorblock.mode === 'remove')

function toggle(piece: (typeof PIECES)[number], value: boolean): void {
  emit('change', { ...props.embed, [piece]: value })
}

function setMode(mode: SponsorMode): void {
  emit('change', { ...props.embed, sponsorblock: { ...props.embed.sponsorblock, mode } })
}

function setCategories(categories: SponsorCategory[]): void {
  emit('change', { ...props.embed, sponsorblock: { ...props.embed.sponsorblock, categories } })
}

function warningText(warning: EmbedWarning): string {
  switch (warning.kind) {
    case 'thumbnail_unsupported':
    case 'chapters_unsupported':
      return t(`linkgrabber.media.embed.warnings.${warning.kind}`, { container: warning.container.toUpperCase() })
    default:
      return t(`linkgrabber.media.embed.warnings.${warning.kind}`)
  }
}
</script>

<template>
  <div class="flex flex-col gap-3" data-testid="media-embed-policy">
    <UFormField :label="t('linkgrabber.media.embed.title')">
      <div class="grid gap-1.5 sm:grid-cols-2">
        <UCheckbox
          v-for="piece in PIECES"
          :key="piece"
          :model-value="props.embed[piece]"
          :label="t(`linkgrabber.media.embed.pieces.${piece}`)"
          :disabled="props.busy || !props.canTranscode"
          :data-testid="`media-embed-${piece}`"
          @update:model-value="(value: boolean | 'indeterminate') => toggle(piece, value === true)"
        />
      </div>
    </UFormField>

    <UFormField :label="t('linkgrabber.media.embed.sponsorblock')">
      <div class="flex flex-wrap items-center gap-1.5">
        <UButton
          v-for="item in modeItems"
          :key="item.value"
          :label="item.label"
          size="xs"
          :color="props.embed.sponsorblock.mode === item.value ? 'primary' : 'neutral'"
          :variant="props.embed.sponsorblock.mode === item.value ? 'soft' : 'ghost'"
          :disabled="props.busy || !props.canTranscode"
          @click="setMode(item.value)"
        />
      </div>
    </UFormField>

    <UAlert
      v-if="destructive"
      color="warning"
      variant="subtle"
      icon="i-lucide-scissors"
      data-testid="media-sponsor-destructive"
      :description="t('linkgrabber.media.embed.remove_hint')"
    />

    <UFormField v-if="!sponsorOff" :label="t('linkgrabber.media.embed.sponsor_categories')">
      <USelectMenu
        :model-value="props.embed.sponsorblock.categories"
        :items="categoryItems"
        value-key="value"
        multiple
        size="xs"
        :disabled="props.busy"
        :placeholder="t('linkgrabber.media.embed.categories.sponsor')"
        data-testid="media-sponsor-categories"
        @update:model-value="setCategories"
      />
    </UFormField>

    <UAlert
      v-for="(warning, index) in props.warnings ?? []"
      :key="index"
      color="warning"
      variant="subtle"
      icon="i-lucide-triangle-alert"
      data-testid="media-embed-warning"
      :description="warningText(warning)"
    />
  </div>
</template>
