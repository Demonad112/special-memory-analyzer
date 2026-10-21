<script setup lang="ts">
import { FolderOpen } from '@lucide/vue'

defineProps<{
  label: string
  hint: string
  disabled?: boolean
  /** A folder is being dragged over this field. */
  dropTarget?: boolean
}>()
const path = defineModel<string>({ required: true })
const emit = defineEmits<{ browse: []; submit: [] }>()
</script>

<template>
  <label
    class="block min-w-0 rounded-lg border bg-card px-4 py-3 transition-colors"
    :class="dropTarget ? 'border-accent bg-accent/8' : 'border-stroke'"
  >
    <span class="flex items-baseline gap-2">
      <span class="font-semibold">{{ label }}</span>
      <span class="truncate text-xs text-muted">{{ hint }}</span>
    </span>
    <span class="mt-2 flex gap-2">
      <input
        v-model="path"
        type="text"
        spellcheck="false"
        autocomplete="off"
        :disabled="disabled"
        :placeholder="dropTarget ? 'Drop the folder here' : 'Type, paste or drop a folder path'"
        class="h-8 min-w-0 flex-1 rounded-md border border-stroke border-b-stroke-strong bg-card px-2.5 text-[13px] outline-none placeholder:text-faint focus:border-b-2 focus:border-b-accent disabled:opacity-60"
        @keydown.enter="emit('submit')"
      />
      <button
        type="button"
        class="flex h-8 shrink-0 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover disabled:opacity-60"
        :disabled="disabled"
        @click.prevent="emit('browse')"
      >
        <FolderOpen
          class="size-4"
          :stroke-width="1.75"
        />
        Browse
      </button>
    </span>
  </label>
</template>
