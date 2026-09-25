<script setup lang="ts">
/**
 * Which stored browser session a private or age-restricted media link is fetched with
 * (RD-080-04).
 *
 * Three states, not a nullable dropdown: "let the scope decide" and "deliberately send
 * nothing" are different intents, and a public video fetched with a session attached is a
 * different request from one fetched without. The automatic choice names the profile it
 * would land on, because "Automatic" on its own tells you nothing about what will be sent.
 *
 * Only profiles that actually cover this link are offered. Pinning one that does not is
 * refused by the server anyway, and offering it here would only move the discovery of that
 * to after the download failed.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AuthProfile, AuthProfileSelection, CandidateAuthProfileMode } from '@/api/types'
import { isUsable, matchFor, scopeLabel } from '@/composables/useAuthProfiles'

const AUTO = '__auto__'
const NONE = '__none__'

const { t } = useI18n()
const props = defineProps<{
  selection: AuthProfileSelection | null
  profiles: AuthProfile[]
  /** The link's address, used to show which profile the automatic choice would pick. */
  url: string
  busy?: boolean
}>()
const emit = defineEmits<{ change: [mode: CandidateAuthProfileMode, profileId?: string] }>()

/** Profiles that carry cookies, are usable, and cover this link. */
const usable = computed(() =>
  props.profiles.filter(
    profile => profile.method === 'cookies' && isUsable(profile) && matchFor([profile], props.url) !== null
  )
)

/** What the server would choose on its own, so "Automatic" can say what it means. */
const automatic = computed(() => matchFor(usable.value, props.url))

const current = computed({
  get(): string {
    const selection = props.selection
    if (!selection || selection.mode === 'auto') return AUTO
    if (selection.mode === 'none') return NONE
    return selection.id ?? AUTO
  },
  set(value: string) {
    if (value === AUTO) emit('change', 'auto')
    else if (value === NONE) emit('change', 'none')
    else emit('change', 'pinned', value)
  }
})

const items = computed(() => [
  {
    value: AUTO,
    label: automatic.value
      ? t('linkgrabber.media.cookies.automaticNamed', { profile: automatic.value.name })
      : t('linkgrabber.media.cookies.automaticNone')
  },
  { value: NONE, label: t('linkgrabber.media.cookies.none') },
  ...usable.value.map(profile => ({
    value: profile.id,
    label: `${profile.name} — ${scopeLabel(profile)}`
  }))
])
</script>

<template>
  <div class="flex flex-col gap-2" data-testid="media-cookie-profile">
    <UFormField
      :label="t('linkgrabber.media.cookies.title')"
      :description="t('linkgrabber.media.cookies.hint')"
    >
      <USelect
        v-model="current"
        size="xs"
        :items="items"
        value-key="value"
        :disabled="props.busy"
        data-testid="media-cookie-select"
      />
    </UFormField>
    <p
      v-if="!usable.length"
      class="text-xs text-muted"
      data-testid="media-cookie-empty"
    >
      {{ t('linkgrabber.media.cookies.empty') }}
    </p>
  </div>
</template>
