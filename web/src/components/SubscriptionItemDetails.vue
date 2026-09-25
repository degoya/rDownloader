<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { hitEpisode, hitSize } from '@/utils/subscriptionHit'

/**
 * What an indexer said about one hit (RD-101-17).
 *
 * The attributes arrive with the search answer the subscription already makes, so nothing
 * here costs a request; only the pictures are fetched, by the browser, when the row is open.
 *
 * Two groups on purpose. The named ones are what actually decides whether a release is the
 * wanted one, so they are laid out and labelled. Everything else is listed raw rather than
 * dropped: indexers disagree about which attributes they emit, and an allow-list would hide
 * exactly the field some indexer considers its most useful one.
 */
const { t } = useI18n()

const props = defineProps<{
  attributes: Record<string, string>
  /** False when external image loading is switched off; the details stay either way. */
  showImages?: boolean
}>()

/** Attributes rendered by a field of their own, so they are not repeated in the raw list. */
const NAMED = [
  'coverurl',
  'backdropcoverurl',
  'imdbplot',
  'imdb',
  'imdbscore',
  'imdbtitle',
  'imdbyear',
  'year',
  'season',
  'episode',
  'resolution',
  'video',
  'audio',
  'size',
  'grabs',
  'seeders',
  'peers',
  'genre',
  'language',
  'password',
]

/**
 * No cover here any more (RD-106-08).
 *
 * The same picture stood in the row as a thumbnail and again here, larger. The row is where
 * the decision is taken, so that is where the large one belongs; `coverurl` stays in `NAMED`
 * so it is not repeated as a raw attribute either. The backdrop below is a different picture
 * -- a wide banner, not a second copy of the cover -- and stays.
 */
const backdrop = computed(() => (props.showImages === false ? null : props.attributes.backdropcoverurl ?? null))
const plot = computed(() => props.attributes.imdbplot ?? null)

/** The short facts, in reading order, skipping whatever this indexer did not send. */
const facts = computed(() => {
  const a = props.attributes
  const out: { label: string, value: string }[] = []
  const add = (key: string, value: string | null | undefined) => {
    if (value) out.push({ label: t(`subscriptions.items.attributes.${key}`), value })
  }

  add('episode', hitEpisode(a))
  add('year', a.imdbyear ?? a.year)
  add('resolution', a.resolution)
  add('video', a.video)
  add('audio', a.audio)
  add('size', hitSize(a))
  add('grabs', a.grabs)
  add('seeders', a.seeders)
  // No `genre` here: the row shows it before expanding, beside the IMDb score and the language,
  // and a fact the row already states is not repeated underneath it.
  return out
})

/** Whatever this indexer sent that has no field of its own. */
const rest = computed(() =>
  Object.entries(props.attributes)
    .filter(([name, value]) => !NAMED.includes(name) && value !== '')
    .map(([name, value]) => ({ name, value })))

const imdbUrl = computed(() => {
  const id = props.attributes.imdb
  // Newznab sends the number without the `tt`, which is not a link on its own.
  return id && /^\d+$/.test(id) ? `https://www.imdb.com/title/tt${id}/` : null
})

/** A picture that does not load leaves nothing behind rather than a broken-image glyph. */
function hide(event: Event) {
  ;(event.target as HTMLElement).hidden = true
}
</script>

<template>
  <div class="flex flex-wrap gap-4">
    <div class="min-w-0 flex-1 space-y-2">
      <dl v-if="facts.length" class="flex flex-wrap gap-x-4 gap-y-1">
        <div v-for="fact in facts" :key="fact.label" class="flex items-baseline gap-1.5">
          <dt class="text-xs text-muted">{{ fact.label }}</dt>
          <dd class="numeric text-xs text-highlighted">{{ fact.value }}</dd>
        </div>
      </dl>

      <p v-if="plot" class="text-xs leading-5 text-muted">{{ plot }}</p>

      <ULink
        v-if="imdbUrl"
        :to="imdbUrl"
        target="_blank"
        rel="noopener noreferrer"
        class="inline-flex items-center gap-1 text-xs"
      >
        <UIcon name="i-lucide-external-link" class="size-3.5" />{{ t('subscriptions.items.imdb') }}
      </ULink>

      <img
        v-if="backdrop"
        :src="backdrop"
        alt=""
        loading="lazy"
        class="max-h-32 w-full bg-elevated object-cover"
        @error="hide"
      >

      <dl v-if="rest.length" class="flex flex-wrap gap-x-4 gap-y-1 border-t border-muted pt-2">
        <div v-for="entry in rest" :key="entry.name" class="flex items-baseline gap-1.5 font-mono">
          <dt class="text-[11px] text-muted">{{ entry.name }}</dt>
          <dd class="text-[11px] break-all text-highlighted">{{ entry.value }}</dd>
        </div>
      </dl>

      <p v-if="!facts.length && !plot && !rest.length" class="text-xs text-muted">
        {{ t('subscriptions.items.no_details') }}
      </p>
    </div>
  </div>
</template>
