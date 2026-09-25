<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SubscriptionItem } from '@/api/types'
import SubscriptionItemActions from '@/components/SubscriptionItemActions.vue'
import {
  cardAspect,
  type CardRatio,
  DEFAULT_CARD_RATIO,
  hitColour,
  hitCover,
  hitEpisode,
  hitGroup,
  hitInitials,
  hitIsMusic,
  hitLocked,
  hitName,
  hitSize,
  hitTitle,
  promotedFacts
} from '@/utils/subscriptionHit'

/**
 * One pending indexer hit as a card in the slider (RD-120-37).
 *
 * Every value comes from `subscriptionHit.ts`, the same reading the list row uses, and whatever
 * the indexer did not send is simply absent. The card nevertheless keeps one height: each band
 * — picture, release name, subtitle, chips — has a fixed size whether it has content or not, so
 * a card without a cover, a size or a group does not make the slider jump.
 *
 * The release name is the card's heading (RD-130-13). A short name above it repeated its head —
 * "The Big Bang Theory" over `The.Big.Bang.Theory.2007.S11E23…` — and made every card taller for
 * it; the name still labels the card for a screen reader and gives the tile its initials.
 *
 * The picture area takes the subscription's ratio (RD-120-42) rather than a fixed height, so it
 * scales with the card's width and a cover fills it without distortion. Every card of one slider
 * has the same width and the same ratio, and the text below keeps a fixed height, so the cards
 * still line up.
 */
const { t } = useI18n()

const props = withDefaults(defineProps<{
  item: SubscriptionItem
  /** False when external image loading is switched off; the initials tile stands in. */
  showImages?: boolean
  /** Whether the details panel under the slider shows this card. */
  selected: boolean
  busy: boolean
  /** The shape of the picture area, as the subscription chose it. */
  ratio?: CardRatio
}>(), { showImages: true, ratio: DEFAULT_CARD_RATIO })
const emit = defineEmits<{
  queue: []
  dismiss: []
  details: []
}>()

const attributes = computed<Record<string, string>>(() => props.item.attributes ?? {})
const coverBroken = ref(false)
const cover = computed(() => (coverBroken.value ? null : hitCover(attributes.value, props.showImages)))
const release = computed(() => hitTitle(props.item.title))
const name = computed(() => hitName(props.item.title, attributes.value))
const initials = computed(() => hitInitials(name.value))
const colour = computed(() => hitColour(name.value))
const music = computed(() => hitIsMusic(props.item.source_category, attributes.value))
const size = computed(() => hitSize(attributes.value))
const locked = computed(() => Boolean(props.item.password) || hitLocked(attributes.value))
const group = computed(() => hitGroup(props.item.title, attributes.value))
const subtitle = computed(() =>
  [hitEpisode(attributes.value), attributes.value.imdbyear ?? attributes.value.year]
    .filter(Boolean)
    .join(' · '))

/** Short facts as chips, in the order they decide: rating, language, picture, genre, grabs. */
const chips = computed(() => {
  const a = attributes.value
  const promoted = Object.fromEntries(promotedFacts(a, t).map(fact => [fact.key, fact]))
  return [
    promoted.imdbscore ? `${promoted.imdbscore.label} ${promoted.imdbscore.value}` : null,
    promoted.language?.value ?? null,
    a.resolution ?? null,
    a.video ?? null,
    promoted.genre?.value ?? null,
    a.grabs ? `${t('subscriptions.items.attributes.grabs')} ${a.grabs}` : null
  ].filter((chip): chip is string => Boolean(chip))
})

const detailsLabel = computed(() =>
  (props.selected ? t('subscriptions.actions.hide_details') : t('subscriptions.actions.details')))
</script>

<template>
  <article
    class="flex min-w-0 flex-col overflow-hidden border bg-default"
    :class="props.selected ? 'border-primary' : 'border-muted'"
    :aria-label="name"
    :aria-roledescription="t('linkgrabber.indexers.cards.slide')"
    data-testid="subscription-card"
  >
    <!-- The cover is positioned out of the flow, so a large picture can never push the area
         past its ratio; it only fills and crops it. -->
    <div
      class="relative flex w-full shrink-0 items-center justify-center overflow-hidden"
      :style="{ backgroundColor: colour, aspectRatio: cardAspect(props.ratio) }"
      :data-ratio="props.ratio"
      data-testid="card-tile"
    >
      <img
        v-if="cover"
        :src="cover"
        alt=""
        loading="lazy"
        decoding="async"
        class="absolute inset-0 size-full object-cover"
        data-testid="card-cover"
        @error="coverBroken = true"
      >
      <span
        v-else-if="initials"
        aria-hidden="true"
        class="text-4xl font-bold tracking-wide text-white"
        data-testid="card-initials"
      >{{ initials }}</span>
      <UIcon
        v-else
        :name="music ? 'i-lucide-music' : 'i-lucide-arrow-down-to-line'"
        class="size-10 text-white"
        aria-hidden="true"
        data-testid="card-symbol"
      />
      <span
        v-if="size"
        class="numeric absolute top-2 right-2 flex items-center gap-1 bg-black/70 px-1.5 py-0.5 font-mono text-[11px] text-white"
      >
        <UIcon
          v-if="locked"
          name="i-lucide-lock"
          class="size-3"
          :aria-label="t('subscriptions.items.password_protected')"
        />{{ size }}
      </span>
      <span
        v-if="group"
        class="absolute bottom-2 left-2 max-w-[80%] truncate bg-black/70 px-1.5 py-0.5 font-mono text-[10px] text-white"
        data-testid="card-group"
      >{{ group }}</span>
    </div>

    <div class="flex h-48 shrink-0 flex-col gap-1 p-3" data-testid="card-body">
      <p class="line-clamp-2 h-10 text-sm leading-5 font-semibold break-all text-highlighted" :title="release" data-testid="card-release">{{ release }}</p>
      <p class="h-4 truncate text-xs text-muted">{{ subtitle }}</p>
      <ul class="flex h-5 flex-wrap gap-1 overflow-hidden">
        <li
          v-for="chip in chips"
          :key="chip"
          class="numeric bg-elevated px-1.5 text-[11px] leading-5 text-highlighted"
        >{{ chip }}</li>
      </ul>
      <p v-if="props.item.password" class="flex items-center gap-1 truncate text-xs text-warning" :title="t('subscriptions.items.password_protected')">
        <UIcon name="i-lucide-key-round" class="size-3.5 shrink-0" />
        <span class="truncate font-mono">{{ props.item.password }}</span>
      </p>

      <div class="mt-auto flex items-center gap-1">
        <SubscriptionItemActions
          compact
          :busy="props.busy"
          @queue="emit('queue')"
          @dismiss="emit('dismiss')"
        />
        <UButton
          size="xs"
          :color="props.selected ? 'primary' : 'neutral'"
          :variant="props.selected ? 'soft' : 'ghost'"
          icon="i-lucide-info"
          class="ml-auto"
          :aria-label="detailsLabel"
          :title="detailsLabel"
          :aria-expanded="props.selected"
          @click="emit('details')"
        />
      </div>
    </div>
  </article>
</template>
