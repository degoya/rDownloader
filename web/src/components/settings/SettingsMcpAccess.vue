<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { CaptureToken } from '@/api/types'
import type { components } from '@/api/schema'
import DataState from '@/components/DataState.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useFetchState } from '@/composables/useFetchState'
import { withBase } from '@/basePath'
import { formatDay } from '@/utils/format'
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
const savingId = ref<string | null>(null)
const editError = ref<string | null>(null)
const confirm = useConfirm()
const toast = useToast()

const mcpEndpoint = `${window.location.origin}${withBase('/mcp')}`
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

/// The areas a chosen one confers on top of itself, named for the preview.
function impliedBy(area: ScopeDescriptor): string[] {
  return area.implies.filter((implied) => !chosen.value.includes(implied))
}

function toggle(scope: string, on: boolean): void {
  chosen.value = on
    ? [...chosen.value, scope]
    : chosen.value.filter((entry) => entry !== scope)
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
    body: { label: pairLabel.value, scopes: chosen.value }
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
  editError.value = null
}

function cancelEdit(): void {
  editingId.value = null
  editScopes.value = []
  editError.value = null
}

function toggleEdit(scope: string, on: boolean): void {
  editScopes.value = on
    ? [...editScopes.value, scope]
    : editScopes.value.filter((entry) => entry !== scope)
}

async function saveScopes(token: CaptureToken): Promise<void> {
  savingId.value = token.id
  editError.value = null
  const response = await api.PATCH('/api/v1/api-tokens/{id}', {
    params: { path: { id: token.id } },
    body: { scopes: editScopes.value }
  })
  savingId.value = null
  if (!response.data) {
    editError.value = responseError(response)
    return
  }
  tokens.value = tokens.value.map(item => (item.id === token.id ? response.data! : item))
  cancelEdit()
  toast.add({ title: t('system.mcp.edit.done'), color: 'success', icon: 'i-lucide-shield-check' })
}

async function copy(value: string, description: string): Promise<void> {
  await navigator.clipboard.writeText(value)
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
  <section :class="embedded ? '' : 'mt-6 border border-muted bg-default p-5'">
    <div class="grid gap-6 lg:grid-cols-[minmax(280px,0.7fr)_minmax(360px,1.3fr)]">
      <div>
        <SectionHeader :eyebrow="t('system.mcp.eyebrow')" :title="t('system.mcp.title')" />
        <i18n-t keypath="system.mcp.description" tag="p" class="mt-2 text-sm leading-6 text-muted">
          <template #endpoint><span class="font-mono">{{ mcpEndpoint }}</span></template>
          <template #scope><span class="font-mono">api:*</span></template>
        </i18n-t>
        <form class="mt-4 space-y-3" @submit.prevent="pair">
          <div class="flex gap-2">
            <UInput v-model="pairLabel" required maxlength="100" icon="i-lucide-monitor-cog" class="flex-1" :placeholder="t('system.mcp.label_placeholder')" />
            <UButton type="submit" icon="i-lucide-key-round" :label="t('system.mcp.submit')" :loading="pairing" :disabled="!chosen.length" />
          </div>
          <fieldset class="space-y-2">
            <legend class="text-sm font-medium text-highlighted">{{ t('system.mcp.scopes_label') }}</legend>
            <p class="text-xs leading-5 text-muted">{{ t('system.mcp.scopes_hint') }}</p>
            <div class="divide-y divide-muted border border-muted">
              <label
                v-for="area in areas"
                :key="area.scope"
                class="flex cursor-pointer items-start gap-3 p-3"
              >
                <UCheckbox
                  :model-value="chosen.includes(area.scope)"
                  @update:model-value="toggle(area.scope, $event === true)"
                />
                <span class="min-w-0 flex-1">
                  <span class="flex flex-wrap items-center gap-2">
                    <span class="text-sm font-medium text-highlighted">{{ areaName(area.scope) }}</span>
                    <UBadge v-if="area.sensitive" color="warning" variant="subtle" size="sm">
                      {{ t('system.mcp.sensitive') }}
                    </UBadge>
                    <span class="numeric text-[11px] text-muted">
                      {{ t('system.mcp.scope_operations', { count: area.operations }) }}
                    </span>
                  </span>
                  <span class="mt-1 block text-xs leading-5 text-muted">
                    {{ t(`system.mcp.areas.${area.scope.replace('api:', '')}.description`) }}
                  </span>
                  <span
                    v-if="chosen.includes(area.scope) && impliedBy(area).length"
                    class="mt-1 block text-xs text-muted"
                  >
                    {{ t('system.mcp.also_includes', { areas: impliedBy(area).map(areaName).join(', ') }) }}
                  </span>
                </span>
              </label>
            </div>
            <UAlert
              v-if="grantsSensitive"
              color="warning"
              variant="subtle"
              icon="i-lucide-triangle-alert"
              :description="t('system.mcp.sensitive_warning')"
            />
            <p v-if="!chosen.length" class="text-xs text-warning">{{ t('system.mcp.scopes_empty') }}</p>
          </fieldset>
        </form>
        <UAlert v-if="pairError" class="mt-3" color="error" variant="subtle" :description="pairError" />
        <div v-if="bearer" class="mt-3 border border-warning/40 bg-warning/10 p-3">
          <p class="mb-2 text-xs font-medium text-warning">{{ t('system.mcp.copy_hint') }}</p>
          <p class="mb-2 text-xs font-medium text-warning">{{ t('system.mcp.token_hint') }}</p>
          <p class="mb-2 text-[11px] text-muted">
            {{ t('system.mcp.minted_areas', { areas: bearerScopes.map(areaName).join(', ') }) }}
          </p>
          <div class="flex items-start gap-2">
            <code class="min-w-0 flex-1 break-all font-mono text-xs leading-5 text-highlighted">{{ bearer }}</code>
            <UButton
              icon="i-lucide-copy"
              :label="t('system.mcp.copy_token')"
              color="neutral"
              variant="soft"
              @click="copy(bearer!, t('system.mcp.token_copied'))"
            />
          </div>
          <div class="mt-3 border-t border-warning/30 pt-3">
            <p class="mb-2 text-xs font-medium text-warning">{{ t('system.mcp.header_hint') }}</p>
            <div class="flex items-start gap-2">
              <code class="min-w-0 flex-1 break-all font-mono text-xs leading-5 text-highlighted">{{ authorizationHeader }}</code>
              <UButton
                icon="i-lucide-copy"
                :label="t('system.mcp.copy_header')"
                color="neutral"
                variant="soft"
                @click="copy(authorizationHeader, t('system.mcp.header_copied'))"
              />
            </div>
            <p class="mt-2 text-[11px] text-muted">{{ t('system.mcp.single_source_hint') }}</p>
          </div>
          <div class="mt-3 border-t border-warning/30 pt-3">
            <p class="mb-2 text-xs font-medium text-warning">{{ t('system.mcp.mcp_hint') }}</p>
            <div class="flex items-start gap-2">
              <code class="min-w-0 flex-1 break-all font-mono text-xs leading-5 text-highlighted">{{ claudeCommand }}</code>
              <UButton
                icon="i-lucide-copy"
                :label="t('system.mcp.copy_command')"
                color="neutral"
                variant="soft"
                @click="copy(claudeCommand, t('system.mcp.command_copied'))"
              />
            </div>
          </div>
        </div>
      </div>
      <div>
        <p class="eyebrow mb-3">{{ t('system.mcp.tokens_eyebrow') }}</p>
        <div v-if="tokens.length" class="divide-y divide-muted border border-muted">
          <div v-for="token in tokens" :key="token.id">
            <div class="flex items-center gap-3 p-3">
              <span class="size-2 bg-success" />
              <div class="min-w-0 flex-1"><p class="truncate text-sm font-medium text-highlighted">{{ token.label }}</p><p class="text-[11px] text-muted">{{ scopeLabel(token) }} · <span class="font-mono">{{ token.scopes.join(', ') }}</span></p></div>
              <span class="numeric text-[11px] text-muted">{{ formatDay(token.created_at) }}</span>
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
            <div v-if="editingId === token.id" class="border-t border-muted bg-elevated/40 p-3">
              <p class="text-xs leading-5 text-muted">{{ t('system.mcp.edit.hint') }}</p>
              <div class="mt-2 divide-y divide-muted border border-muted bg-default">
                <label
                  v-for="area in areas"
                  :key="area.scope"
                  class="flex cursor-pointer items-start gap-3 p-2"
                >
                  <UCheckbox
                    :model-value="editScopes.includes(area.scope)"
                    @update:model-value="toggleEdit(area.scope, $event === true)"
                  />
                  <span class="min-w-0 flex-1">
                    <span class="flex flex-wrap items-center gap-2">
                      <span class="text-sm font-medium text-highlighted">{{ areaName(area.scope) }}</span>
                      <UBadge v-if="area.sensitive" color="warning" variant="subtle" size="sm">
                        {{ t('system.mcp.sensitive') }}
                      </UBadge>
                      <span class="numeric text-[11px] text-muted">
                        {{ t('system.mcp.scope_operations', { count: area.operations }) }}
                      </span>
                    </span>
                  </span>
                </label>
              </div>
              <UAlert
                v-if="editGrantsSensitive"
                class="mt-2"
                color="warning"
                variant="subtle"
                icon="i-lucide-triangle-alert"
                :description="t('system.mcp.sensitive_warning')"
              />
              <p v-if="!editScopes.length" class="mt-2 text-xs text-warning">{{ t('system.mcp.edit.empty') }}</p>
              <UAlert v-if="editError" class="mt-2" color="error" variant="subtle" :description="editError" />
              <div class="mt-3 flex gap-2">
                <UButton
                  size="xs"
                  icon="i-lucide-shield-check"
                  :label="t('system.mcp.edit.save')"
                  :loading="savingId === token.id"
                  :disabled="!editScopes.length"
                  @click="saveScopes(token)"
                />
                <UButton
                  size="xs"
                  color="neutral"
                  variant="ghost"
                  :label="t('system.mcp.edit.cancel')"
                  @click="cancelEdit()"
                />
              </div>
            </div>
          </div>
        </div>
        <DataState v-else :loading="loading" :error="loadError" :empty="true" :rows="2">
          <p class="border border-dashed border-muted p-6 text-center text-sm text-muted">{{ t('system.mcp.empty') }}</p>
        </DataState>
      </div>
    </div>
  </section>
</template>
