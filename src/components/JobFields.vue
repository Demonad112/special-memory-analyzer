<script setup lang="ts">
import { job } from '@/lib/job'

defineProps<{ disabled?: boolean }>()

const fields = [
  { key: 'client', label: 'Client', placeholder: 'Company or person' },
  { key: 'ticket', label: 'Ticket', placeholder: 'Ticket or job number' },
  { key: 'technician', label: 'Technician', placeholder: 'Your name' },
] as const
</script>

<template>
  <fieldset class="grid grid-cols-3 gap-2">
    <legend class="sr-only">Job details for the report</legend>
    <label
      v-for="field in fields"
      :key="field.key"
      class="min-w-0"
    >
      <span class="mb-1 block text-xs text-muted">{{ field.label }}</span>
      <input
        v-model="job[field.key]"
        type="text"
        spellcheck="false"
        autocomplete="off"
        :disabled="disabled"
        :placeholder="field.placeholder"
        class="h-8 w-full min-w-0 rounded-md border border-stroke border-b-stroke-strong bg-card px-2.5 text-[13px] outline-none placeholder:text-faint focus:border-b-2 focus:border-b-accent disabled:opacity-60"
      />
    </label>
  </fieldset>
</template>
