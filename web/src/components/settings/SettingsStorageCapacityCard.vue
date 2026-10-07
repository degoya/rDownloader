<script setup lang="ts">
/**
 * The global storage capacity (RD-1120-21, from *General*), beside the storage roots whose own
 * "minimum free" overrides it: the threshold, the headroom for unknown sizes, the collision rule
 * and the automatic resume.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { CollisionPolicy } from '@/api/storage'
import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import CollisionPolicySelect from '@/components/storage/CollisionPolicySelect.vue'
import { GIB, byteModel } from '@/utils/format'
import NumberWithUnit from '@/components/NumberWithUnit.vue'
import { DECIMAL, WHOLE } from '@/utils/numberInput'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()

/** The free-space threshold is entered in GiB; the API stores raw bytes. */
const minimumFreeGiB = byteModel(
  () => settings.value.storage_minimum_free_bytes,
  (raw) => { settings.value.storage_minimum_free_bytes = raw ?? '0' },
  GIB,
  '0'
)

/** The global level always has a policy; the select's `null` (inherit) is never offered here. */
const collisionPolicy = computed<CollisionPolicy | null>({
  get: () => settings.value.storage_collision_policy,
  set: (value) => { if (value) settings.value.storage_collision_policy = value }
})
</script>

<template>
  <UCard as="section" :ui="{ body: 'grid gap-4' }" data-settings-anchor="routing.storage_capacity">
    <SectionHeader :eyebrow="t('settings.storage.eyebrow')" :title="t('settings.storage.title')" :description="t('settings.storage.description')" />
    <UFormField data-settings-anchor="routing.minimum_free" :label="t('settings.storage.minimum_free.label')" :description="t('settings.storage.minimum_free.description')">
      <NumberWithUnit v-model="minimumFreeGiB" unit="GiB" :min="0" :format-options="DECIMAL" :step-snapping="false" class="mt-2 w-full" />
    </UFormField>
    <UFormField :label="t('settings.storage.headroom.label')" :description="t('settings.storage.headroom.description')">
      <UInputNumber v-model="settings.storage_unknown_size_headroom" required :min="1" :max="64" :format-options="WHOLE" class="mt-2 w-full" />
    </UFormField>
    <UFormField data-settings-anchor="routing.collision" :label="t('settings.storage.collision.label')" :description="t('settings.storage.collision.description')">
      <CollisionPolicySelect v-model="collisionPolicy" class="mt-2" />
    </UFormField>
    <UFormField :label="t('settings.storage.auto_resume.label')" :description="t('settings.storage.auto_resume.description')" orientation="horizontal">
      <USwitch v-model="settings.storage_auto_resume" />
    </UFormField>
  </UCard>
</template>
