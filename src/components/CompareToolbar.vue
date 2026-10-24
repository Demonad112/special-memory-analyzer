<script setup lang="ts">
import { ClipboardList, Copy, FileText, History } from '@lucide/vue'
import type { ComparePaths, CompareResult } from '@/lib/compare'
import type { CompareFilter } from '@/lib/diffRows'
import { formatBytes, formatDuration, formatTime, plural } from '@/lib/format'

defineProps<{
  result: CompareResult
  compared: ComparePaths
  finishedAt: Date | null
  filter: CompareFilter
  /** Whether the "Copy missing files" confirm panel is open. */
  confirming: boolean
}>()
const emit = defineEmits<{
  filter: [filter: CompareFilter]
  report: []
  copyList: []
  copyFiles: []
  confirming: [open: boolean]
  recheck: []
}>()

const filters: { value: CompareFilter; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'changes', label: 'Differences' },
  { value: 'missing', label: 'Missing' },
]
</script>

<template>
  <div class="mt-4 flex items-center gap-3">
    <div
      class="flex shrink-0 rounded-md border border-stroke bg-subtle p-0.5"
      role="radiogroup"
      aria-label="Show"
    >
      <button
        v-for="option in filters"
        :key="option.value"
        type="button"
        role="radio"
        :aria-checked="filter === option.value"
        class="flex h-7 items-center rounded px-3 text-[13px]"
        :class="
          filter === option.value ? 'bg-accent text-on-accent' : 'text-fg hover:bg-subtle-strong'
        "
        @click="emit('filter', option.value)"
      >
        {{ option.label }}
      </button>
    </div>
    <p
      class="min-w-0 flex-1 truncate text-right text-xs text-muted tabular-nums"
      :title="`${compared.left} → ${compared.right}`"
    >
      Compared
      {{ finishedAt && formatTime(finishedAt) }}
      in {{ formatDuration(result.elapsedMs) }}
    </p>
    <button
      v-if="result.reportId"
      type="button"
      class="flex h-8 shrink-0 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover"
      title="Open the saved report in the browser (print or save as PDF from there). Export it from Reports."
      @click="emit('report')"
    >
      <FileText
        class="size-4"
        :stroke-width="1.75"
      />
      Report
    </button>
    <button
      v-if="result.summary.missing > 0"
      type="button"
      class="flex h-8 shrink-0 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover"
      title="Copy every missing file's path, with a short header, to paste into an email or ticket"
      @click="emit('copyList')"
    >
      <ClipboardList
        class="size-4"
        :stroke-width="1.75"
      />
      Copy missing list
    </button>
    <button
      v-if="result.summary.missing > 0"
      type="button"
      class="flex h-8 shrink-0 items-center gap-1.5 rounded-md bg-accent px-3 text-[13px] text-on-accent hover:bg-accent-hover"
      title="Copy the missing files to the destination as a record, with progress, a report and retry"
      @click="emit('confirming', true)"
    >
      <Copy
        class="size-4"
        :stroke-width="1.75"
      />
      Copy missing files
    </button>
    <button
      type="button"
      class="flex h-8 shrink-0 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover"
      title="Compare the same two folders again and see what changed"
      @click="emit('recheck')"
    >
      <History
        class="size-4"
        :stroke-width="1.75"
      />
      Re-check
    </button>
  </div>
  <div
    v-if="confirming"
    class="mt-3 flex flex-wrap items-center gap-3 rounded-lg border border-stroke bg-card px-4 py-3 text-[13px]"
  >
    <p class="min-w-0 flex-1">
      Copy {{ plural(result.summary.missing, 'missing file') }} ({{
        formatBytes(result.summary.missingBytes)
      }}) from <strong class="break-all">{{ compared.left }}</strong> to
      <strong class="break-all">{{ compared.right }}</strong
      >? Files already there are never overwritten. Dates and attributes are kept.
    </p>
    <button
      type="button"
      class="h-8 rounded-md bg-accent px-3 text-on-accent hover:bg-accent-hover"
      @click="emit('copyFiles')"
    >
      Copy
    </button>
    <button
      type="button"
      class="h-8 rounded-md border border-stroke px-3 hover:bg-card-hover"
      @click="emit('confirming', false)"
    >
      Cancel
    </button>
  </div>
</template>
