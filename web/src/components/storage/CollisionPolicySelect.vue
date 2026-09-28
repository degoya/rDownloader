<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { COLLISION_POLICIES, type CollisionPolicy } from '@/api/storage'
import { INHERIT_LEVEL } from '@/utils/format'

/**
 * The five collision policies (RD-150-01), with the level's "inherit" in front when the level
 * may have none of its own. `null` is that inherit; the hint below names what the chosen policy
 * does, so the choice is never a bare word.
 */
const props = defineProps<{
  /** Label of the inherit entry; absent for the global level, which always has a policy. */
  inheritLabel?: string
  disabled?: boolean
}>()
const model = defineModel<CollisionPolicy | null>({ required: true })
const { t } = useI18n()

const items = computed(() => [
  ...(props.inheritLabel ? [{ label: props.inheritLabel, value: INHERIT_LEVEL }] : []),
  ...COLLISION_POLICIES.map(policy => ({ label: t(`downloads.collision.policies.${policy}`), value: policy }))
])

const selected = computed<string>({
  get: () => model.value ?? INHERIT_LEVEL,
  set: (value) => { model.value = value === INHERIT_LEVEL ? null : value as CollisionPolicy }
})
</script>

<template>
  <div>
    <USelect v-model="selected" :items="items" value-key="value" :disabled="props.disabled" class="w-full" data-testid="collision-policy-select" />
    <p v-if="model" class="mt-1.5 text-xs leading-5 text-muted">{{ t(`downloads.collision.hints.${model}`) }}</p>
  </div>
</template>
