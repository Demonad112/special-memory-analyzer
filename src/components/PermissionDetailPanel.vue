<script setup lang="ts">
import { computed } from 'vue'
import type { PermCell, PermissionDetail, PermissionEntry } from '@/lib/compare'

const props = defineProps<{
  name: string
  cell: PermCell
  /** `null` when the report limit left this item's details out. */
  detail: PermissionDetail | null
  loading: boolean
}>()

function entryText(entry: PermissionEntry): string {
  return `${entry.account}: ${entry.access} ${entry.rights} (${entry.appliesTo})${entry.inherited ? ', inherited' : ''}`
}

const inheritance = computed(() => {
  const detail = props.detail
  if (!detail || detail.sourceProtected === detail.destinationProtected) {
    return ''
  }
  const word = (protectedFlag: boolean): string => (protectedFlag ? 'off' : 'on')
  return `Inheriting from the parent: ${word(detail.sourceProtected)} at the source, ${word(detail.destinationProtected)} at the destination`
})
</script>

<template>
  <section
    class="mt-3 rounded-lg border border-stroke bg-card px-4 py-3 text-[13px]"
    aria-live="polite"
    data-testid="permission-detail"
  >
    <p class="font-semibold">Permissions of {{ name }}</p>
    <p
      v-if="loading"
      class="mt-1 text-muted"
    >
      Loading…
    </p>
    <p
      v-else-if="!detail"
      class="mt-1 text-muted"
    >
      {{
        cell.summary ||
        (cell.below > 0
          ? `${String(cell.below)} item(s) below this folder differ.`
          : 'The details of this item were not kept (the report limit was reached).')
      }}
    </p>
    <ul
      v-else
      class="mt-1 space-y-0.5"
    >
      <li v-if="detail.error">{{ detail.error }}</li>
      <li v-if="detail.sourceOwner || detail.destinationOwner">
        Owner: {{ detail.sourceOwner || '(none)' }} at the source,
        {{ detail.destinationOwner || '(none)' }} at the destination
      </li>
      <li v-if="inheritance">{{ inheritance }}</li>
      <li v-if="detail.note">{{ detail.note }}</li>
      <li
        v-for="entry in detail.missing"
        :key="`m-${entry.sid}-${String(entry.mask)}-${entry.appliesTo}-${entry.access}`"
        class="text-danger"
      >
        Missing at the destination: {{ entryText(entry) }}
      </li>
      <li
        v-for="entry in detail.extra"
        :key="`x-${entry.sid}-${String(entry.mask)}-${entry.appliesTo}-${entry.access}`"
        class="text-danger"
      >
        Only at the destination: {{ entryText(entry) }}
      </li>
      <li
        v-for="pair in detail.sameName"
        :key="`s-${pair.source.sid}-${pair.destination.sid}-${String(pair.source.mask)}`"
        class="text-warning"
      >
        Same name, different account: {{ entryText(pair.source) }} ↔ {{ pair.destination.account }}
      </li>
    </ul>
  </section>
</template>
