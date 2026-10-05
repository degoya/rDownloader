import { computed, ref, type Ref } from 'vue'

import { translateServerMessage } from '@/i18n/server'
import { useIndexersStore } from '@/stores/indexers'
import { answers, emptyTypedFields, offeredFields, typedBody, typedProblem, type TypedFields } from '@/utils/indexerSearchType'

/**
 * The TV and film searches of the LinkGrabber's indexer search (RD-1100-03): the fields, their
 * check, and which of the chosen indexers a typed search goes to.
 *
 * `targetIds` are the indexers the search would ask as a free search. A typed one asks only those
 * whose `t=caps` say they answer it — waiting for the answers first, so a search sent right after
 * choosing a type is still sent to the right ones — and nothing when none does.
 */
export function useIndexerSearchType(targetIds: Ref<string[]>) {
  const indexers = useIndexersStore()
  const typed = ref<TypedFields>(emptyTypedFields())
  const typedError = ref<string | null>(null)
  const capsList = computed(() => targetIds.value.map(id => indexers.caps[id]))
  /** The ids the chosen type sends: the ones its fields show. */
  const sentFields = computed(() => offeredFields(typed.value.type, capsList.value))

  function askCaps(): void {
    void indexers.loadCaps(targetIds.value)
  }

  /** Checks the ids; true when they may be sent. */
  function validateTyped(): boolean {
    const fault = typedProblem(typed.value, sentFields.value)
    typedError.value = fault ? translateServerMessage(fault) : null
    return !typedError.value
  }

  /** The indexers a typed search asks, or `null` when none of the chosen ones answers it. */
  async function answeringIndexers(): Promise<string[] | null> {
    const type = typed.value.type
    await indexers.loadCaps(targetIds.value)
    const answering = targetIds.value.filter(id => answers(indexers.caps[id], type))
    return answering.length ? answering : null
  }

  /** The typed part of the request body; empty for a free search. */
  function typedPart(): Record<string, string | number> {
    return typedBody(typed.value, sentFields.value)
  }

  return { typed, typedError, capsList, askCaps, validateTyped, answeringIndexers, typedPart }
}
