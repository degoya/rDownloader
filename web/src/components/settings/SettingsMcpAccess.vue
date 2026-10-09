<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { CaptureToken } from '@/api/types'
import type { components } from '@/api/schema'
import CopyField from '@/components/CopyField.vue'
import DataState from '@/components/DataState.vue'
import FormActions from '@/components/FormActions.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import NumberWithUnit from '@/components/NumberWithUnit.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useFetchState } from '@/composables/useFetchState'
import { serviceUrl } from '@/basePath'
import { formatDay } from '@/utils/format'
import { WHOLE, orNull } from '@/utils/numberInput'
import { expiresInDays, tokenExpired, tokenExpiryItems, tokenExpiryLabel } from '@/utils/tokenExpiry'
import SectionHeader from '@/components/SectionHeader.vue'

defineProps<{ embedded?: boolean }>()

type ScopeDescriptor = components['schemas']['ScopeDescriptor']

const { t } = useI18n()
const tokens = ref<CaptureToken[]>([])
const pairLabel = ref('')
/// The areas a token can hold, with how much each reaches. Read from the server rather than
/// listed here, so the numbers a person chooses against come from the table that enforces
/// them and cannot drift as routes are added.
const areas = ref<ScopeDescriptor[]>([])
/// Least privilege by default. A form that opens on "everything" is a form whose default
/// everybody keeps, which would make the areas decorative.
const chosen = ref<string[]>(['api:read'])
/// Days until the new token expires; `0`, never, is the default (RD-1110-07).
const expiryDays = ref(0)
const expiryItems = computed(() => tokenExpiryItems(t))
/// Calls per minute of the new token, REST and MCP together; empty is no limit (RD-1200-04).
const rateLimit = ref<number | null>(null)
const bearer = ref<string | null>(null)
const bearerTokenId = ref<string | null>(null)
/// Areas of the token currently shown, so the hint describes what was actually minted rather
/// than the state of the form after it was changed again.
const bearerScopes = ref<string[]>([])
const pairError = ref<string | null>(null)
const pairing = ref(false)
const revokingId = ref<string | null>(null)
/// The token whose areas are open for editing, and the set as it currently stands in that
/// editor. Held beside the list rather than inside the row so cancelling restores what the
/// server has, not what was typed.
const editingId = ref<string | null>(null)
const editScopes = ref<string[]>([])
/// The call limit in that editor, saved beside the areas when it changed.
const editRate = ref<number | null>(null)
const savingId = ref<string | null>(null)
const editError = ref<string | null>(null)
const confirm = useConfirm()
const toast = useToast()

const mcpEndpoint = serviceUrl('/mcp')
// Shown for every token: the MCP transport accepts any API area, and which of the sixteen
// tools a token may call is decided per tool. Hiding the command for a reading token used to
// be right and is not any more.
const claudeCommand = computed(() => bearer.value
  ? `claude mcp add --transport http rdownloader ${mcpEndpoint} --header "Authorization: Bearer ${bearer.value}"`
  : '')

// The complete header *value*, because that is what a connector dialog asks for. Clients that
// read the header out of an environment variable — the ChatGPT desktop app among them — want
// `Bearer <token>` in the variable, and offering only the raw token invites the two failures
// that are indistinguishable from a broken server afterwards: the `Bearer ` prefix left off,
// or the token pasted into a field that wanted the variable's *name*.
const authorizationHeader = computed(() => (bearer.value ? `Bearer ${bearer.value}` : ''))

// Deliberately no combined total. The server reports what each area reaches on its own, and
// those figures overlap — everything that acts also reads — so adding them overstates and
// taking the largest understates. An understating preview is the worse of the two failures,
// and neither is worth inventing when the per-area numbers are already the honest answer.
const grantsSensitive = computed(() =>
  areas.value.some((area) => area.sensitive && chosen.value.includes(area.scope))
)

// The same warning for the editor, because widening an existing token is the more dangerous
// of the two moments: the client already holds the value, so the new area is in force at its
// very next request.
const editGrantsSensitive = computed(() =>
  areas.value.some((area) => area.sensitive && editScopes.value.includes(area.scope))
)

/** The paired-agent list's own fetch; `pairing` above belongs to the form (RD-104-07). */
const { loading, loadError, load } = useFetchState()

onMounted(() => {
  void load(loadTokens)
  void loadAreas()
})

async function loadAreas(): Promise<void> {
  const response = await api.GET('/api/v1/api-tokens/scopes')
  if (response.data) areas.value = response.data
}

/// The areas as `UCheckboxGroup` items: the group carries the fieldset, the legend and the
/// checked state that the hand-built rows of labels had to assemble themselves (RD-150-11).
const scopeItems = computed(() => areas.value.map((area) => ({
  label: areaName(area.scope),
  value: area.scope,
  description: t(`system.mcp.areas.${area.scope.replace('api:', '')}.description`),
  sensitive: area.sensitive,
  operations: area.operations,
  implied: impliedBy(area)
})))
/// The same areas under a token's row, where the hint above the group replaces the long texts.
const editScopeItems = computed(() => scopeItems.value.map(({ description: _description, ...item }) => item))

/// The areas a chosen one confers on top of itself, named for the preview.
function impliedBy(area: ScopeDescriptor): string[] {
  return area.implies.filter((implied) => !chosen.value.includes(implied))
}

function areaName(scope: string): string {
  return t(`system.mcp.areas.${scope.replace('api:', '')}.name`)
}

async function loadTokens(): Promise<string | null> {
  const response = await api.GET('/api/v1/api-tokens')
  if (!response.data) return responseError(response)
  tokens.value = response.data
  return null
}

async function pair(): Promise<void> {
  pairing.value = true
  pairError.value = null
  const response = await api.POST('/api/v1/api-tokens', {
    body: {
      label: pairLabel.value,
      scopes: chosen.value,
      expires_in_days: expiresInDays(expiryDays.value),
      calls_per_minute: orNull(rateLimit.value)
    }
  })
  pairing.value = false
  if (response.data) {
    bearer.value = response.data.bearer
    bearerScopes.value = [...chosen.value]
    bearerTokenId.value = response.data.token.id
    tokens.value = [response.data.token, ...tokens.value]
  } else {
    pairError.value = responseError(response)
  }
}

function startEdit(token: CaptureToken): void {
  editingId.value = token.id
  // `api:*` is not one of the six checkboxes; a token holding it opens on every area ticked,
  // which is the same permission written the way this form can express it.
  editScopes.value = token.scopes.includes('api:*')
    ? areas.value.map((area) => area.scope)
    : token.scopes.filter((scope) => areas.value.some((area) => area.scope === scope))
  editRate.value = token.calls_per_minute ?? null
  editError.value = null
}

function cancelEdit(): void {
  editingId.value = null
  editScopes.value = []
  editRate.value = null
  editError.value = null
}

async function saveScopes(token: CaptureToken): Promise<void> {
  savingId.value = token.id
  editError.value = null
  const response = await api.PATCH('/api/v1/api-tokens/{id}', {
    params: { path: { id: token.id } },
    body: { scopes: editScopes.value }
  })
  if (!response.data) {
    savingId.value = null
    editError.value = responseError(response)
    return
  }
  let saved = response.data
  // The limit has a route of its own (RD-1200-04), asked only when it changed.
  const limit = orNull(editRate.value)
  if (limit !== (token.calls_per_minute ?? null)) {
    const limited = await api.PUT('/api/v1/api-tokens/{id}/limits', {
      params: { path: { id: token.id } },
      body: { calls_per_minute: limit }
    })
    if (!limited.data) {
      savingId.value = null
      tokens.value = tokens.value.map(item => (item.id === token.id ? saved : item))
      editError.value = responseError(limited)
      return
    }
    saved = limited.data
  }
  savingId.value = null
  tokens.value = tokens.value.map(item => (item.id === token.id ? saved : item))
  cancelEdit()
  toast.add({ title: t('system.mcp.edit.done'), color: 'success', icon: 'i-lucide-shield-check' })
}

function copied(description: string): void {
  toast.add({
    title: t('system.mcp.copied_title'),
    description,
    color: 'success',
    icon: 'i-lucide-copy-check'
  })
}

async function revokeToken(token: CaptureToken): Promise<void> {
  const confirmed = await confirm({
    title: t('system.mcp.revoke.title'),
    description: t('system.mcp.revoke.description', { label: token.label }),
    confirmLabel: t('system.mcp.revoke.confirm'),
    confirmIcon: 'i-lucide-unplug',
    destructive: true
  })
  if (!confirmed) return
  revokingId.value = token.id
  const response = await api.DELETE('/api/v1/api-tokens/{id}', {
    params: { path: { id: token.id } }
  })
  revokingId.value = null
  if (!response.data) {
    pairError.value = responseError(response)
    return
  }
  tokens.value = tokens.value.filter(item => item.id !== token.id)
  if (bearerTokenId.value === token.id) {
    bearer.value = null
    bearerTokenId.value = null
  }
  toast.add({ title: t('system.mcp.revoke.done'), color: 'success', icon: 'i-lucide-unplug' })
}

/// Names the areas a listed token holds, so what it can do is readable without knowing what
/// the raw scope strings mean.
function scopeLabel(token: CaptureToken): string {
  if (token.scopes.includes('api:*')) return t('system.mcp.scope_full')
  const named = token.scopes.filter((scope) => scope.startsWith('api:')).map(areaName)
  return named.length ? named.join(', ') : t('system.mcp.scope_none')
}
</script>

<template>
  <UCard as="section" data-settings-anchor="clients.api" :ui="embedded ? { root: 'overflow-visible rounded-none bg-transparent', body: 'p-0 sm:p-0' } : undefined">
    <FormListLayout :list-title="t('system.mcp.tokens_eyebrow')" :count="tokens.length">
      <template #form>
        <SectionHeader :eyebrow="t('system.mcp.eyebrow')" :title="t('system.mcp.title')">
          <template #description>
            <i18n-t keypath="system.mcp.description" tag="span">
              <template #endpoint><span class="font-mono">{{ mcpEndpoint }}</span></template>
              <template #scope><span class="font-mono">api:*</span></template>
            </i18n-t>
          </template>
        </SectionHeader>
        <UAlert v-if="pairError" class="mt-4" color="error" :description="pairError" />
        <form class="mt-4 space-y-3" @submit.prevent="pair">
          <UFormField :label="t('system.mcp.label_label')" required>
            <UInput v-model="pairLabel" required maxlength="100" icon="i-lucide-monitor-cog" class="w-full" :placeholder="t('system.mcp.label_placeholder')" />
          </UFormField>
          <div class="space-y-2">
            <p class="text-xs leading-5 text-muted">{{ t('system.mcp.scopes_hint') }}</p>
            <UCheckboxGroup v-model="chosen" :items="scopeItems" :legend="t('system.mcp.scopes_label')" variant="table">
              <template #label="{ item }">
                <span class="flex flex-wrap items-center gap-2">
                  <span class="text-sm font-medium text-highlighted">{{ item.label }}</span>
                  <UBadge v-if="item.sensitive" color="warning" variant="subtle" size="sm">{{ t('system.mcp.sensitive') }}</UBadge>
                  <span class="numeric text-2xs text-muted">{{ t('system.mcp.scope_operations', { count: item.operations }) }}</span>
                </span>
              </template>
              <template #description="{ item }">
                <span class="block text-xs leading-5 text-muted">{{ item.description }}</span>
                <span v-if="chosen.includes(item.value) && item.implied.length" class="mt-1 block text-xs text-muted">
                  {{ t('system.mcp.also_includes', { areas: item.implied.map(areaName).join(', ') }) }}
                </span>
              </template>
            </UCheckboxGroup>
            <UAlert
              v-if="grantsSensitive"
              color="warning"
              icon="i-lucide-triangle-alert"
              :description="t('system.mcp.sensitive_warning')"
            />
            <p v-if="!chosen.length" class="text-xs text-warning">{{ t('system.mcp.scopes_empty') }}</p>
          </div>
          <UFormField :label="t('system.token_expiry.label')" :description="t('system.token_expiry.hint')">
            <USelect v-model="expiryDays" :items="expiryItems" icon="i-lucide-calendar-clock" class="w-full" data-testid="token-expiry" />
          </UFormField>
          <UFormField :label="t('system.token_rate.label')" :description="t('system.token_rate.hint')">
            <NumberWithUnit
              v-model="rateLimit"
              :unit="t('system.token_rate.unit')"
              :min="1"
              :max="6000"
              :format-options="WHOLE"
              :placeholder="t('system.token_rate.none')"
              class="w-full"
              data-testid="token-rate"
            />
          </UFormField>
          <FormActions :create-label="t('system.mcp.submit')" create-icon="i-lucide-key-round" :loading="pairing" :disabled="!chosen.length" />
        </form>
        <UAlert v-if="bearer" class="mt-3" color="warning" :title="t('system.mcp.copy_hint')">
          <template #description>
            <p class="mb-2 text-xs font-medium text-warning">{{ t('system.mcp.token_hint') }}</p>
            <p class="mb-2 text-2xs text-muted">
              {{ t('system.mcp.minted_areas', { areas: bearerScopes.map(areaName).join(', ') }) }}
            </p>
            <CopyField :value="bearer!" :label="t('system.mcp.copy_token')" @copied="copied(t('system.mcp.token_copied'))" />
            <USeparator class="my-3" :ui="{ border: 'border-warning/30' }" />
            <p class="mb-2 text-xs font-medium text-warning">{{ t('system.mcp.header_hint') }}</p>
            <CopyField :value="authorizationHeader" :label="t('system.mcp.copy_header')" @copied="copied(t('system.mcp.header_copied'))" />
            <p class="mt-2 text-2xs text-muted">{{ t('system.mcp.single_source_hint') }}</p>
            <USeparator class="my-3" :ui="{ border: 'border-warning/30' }" />
            <p class="mb-2 text-xs font-medium text-warning">{{ t('system.mcp.mcp_hint') }}</p>
            <CopyField :value="claudeCommand" :label="t('system.mcp.copy_command')" @copied="copied(t('system.mcp.command_copied'))" />
          </template>
        </UAlert>
      </template>
      <template #list>
        <div v-if="tokens.length" class="divide-y divide-muted border border-muted">
          <div v-for="token in tokens" :key="token.id">
            <div class="flex items-center gap-3 p-3">
              <UChip standalone :color="tokenExpired(token.expires_at) ? 'error' : 'success'" />
              <div class="min-w-0 flex-1">
                <p class="truncate text-sm font-medium text-highlighted">{{ token.label }}</p>
                <p class="text-2xs text-muted">{{ scopeLabel(token) }} · <span class="font-mono">{{ token.scopes.join(', ') }}</span></p>
                <p v-if="token.expires_at" class="text-2xs" :class="tokenExpired(token.expires_at) ? 'text-error' : 'text-muted'">{{ tokenExpiryLabel(token.expires_at, t) }}</p>
                <p v-if="token.calls_per_minute" class="numeric text-2xs text-muted">{{ t('system.token_rate.row', { count: token.calls_per_minute }) }}</p>
              </div>
              <span class="numeric text-2xs text-muted">{{ formatDay(token.created_at) }}</span>
              <UButton
                icon="i-lucide-pencil"
                :aria-label="t('system.mcp.edit.label')"
                :title="t('system.mcp.edit.label')"
                color="neutral"
                variant="ghost"
                size="xs"
                @click="editingId === token.id ? cancelEdit() : startEdit(token)"
              />
              <UButton
                icon="i-lucide-trash-2"
                :aria-label="t('system.mcp.revoke_label')"
                :title="t('system.mcp.revoke_label')"
                color="error"
                variant="ghost"
                size="xs"
                :loading="revokingId === token.id"
                @click="revokeToken(token)"
              />
            </div>
            <!-- The areas are edited under the row rather than in the form beside the list: the
                 form mints a new token, and a change here keeps the token's value, which the
                 two places make visible. -->
            <form v-if="editingId === token.id" class="border-t border-muted bg-elevated/40 p-3" @submit.prevent="saveScopes(token)">
              <p class="text-xs leading-5 text-muted">{{ t('system.mcp.edit.hint') }}</p>
              <UCheckboxGroup
                v-model="editScopes"
                class="mt-2"
                :items="editScopeItems"
                :legend="t('system.mcp.scopes_label')"
                variant="table"
                size="sm"
              >
                <template #label="{ item }">
                  <span class="flex flex-wrap items-center gap-2">
                    <span class="text-sm font-medium text-highlighted">{{ item.label }}</span>
                    <UBadge v-if="item.sensitive" color="warning" variant="subtle" size="sm">{{ t('system.mcp.sensitive') }}</UBadge>
                    <span class="numeric text-2xs text-muted">{{ t('system.mcp.scope_operations', { count: item.operations }) }}</span>
                  </span>
                </template>
              </UCheckboxGroup>
              <UAlert
                v-if="editGrantsSensitive"
                class="mt-2"
                color="warning"
                icon="i-lucide-triangle-alert"
                :description="t('system.mcp.sensitive_warning')"
              />
              <p v-if="!editScopes.length" class="mt-2 text-xs text-warning">{{ t('system.mcp.edit.empty') }}</p>
              <UFormField class="mt-3" :label="t('system.token_rate.label')" :description="t('system.token_rate.hint')">
                <NumberWithUnit
                  v-model="editRate"
                  :unit="t('system.token_rate.unit')"
                  :min="1"
                  :max="6000"
                  :format-options="WHOLE"
                  :placeholder="t('system.token_rate.none')"
                  class="w-full"
                  data-testid="token-edit-rate"
                />
              </UFormField>
              <UAlert v-if="editError" class="mt-2" color="error" :description="editError" />
              <FormActions
                class="mt-3"
                editing
                :create-label="t('system.mcp.edit.save')"
                :save-label="t('system.mcp.edit.save')"
                :loading="savingId === token.id"
                :disabled="!editScopes.length"
                @cancel="cancelEdit()"
              />
            </form>
          </div>
        </div>
        <DataState v-else :loading="loading" :error="loadError" :empty="true" :rows="2">
          <UEmpty :description="t('system.mcp.empty')" />
        </DataState>
      </template>
    </FormListLayout>
  </UCard>
</template>
