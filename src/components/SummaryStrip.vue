<script setup lang="ts">
import { computed } from 'vue'
import type { DiffSummary } from '@/lib/compare'
import type { CompareFilter } from '@/lib/diffRows'
import { formatBytes, formatDelta, plural } from '@/lib/format'

const props = defineProps<{ summary: DiffSummary; filter: CompareFilter }>()
const emit = defineEmits<{ filter: [filter: CompareFilter] }>()

const delta = computed(() => props.summary.right.size - props.summary.left.size)
const deltaText = computed(() => (delta.value === 0 ? 'same size' : formatDelta(delta.value)))
const hasMissing = computed(() => props.summary.missing > 0)
const hasOther = computed(() => props.summary.different + props.summary.extra > 0)
</script>

<template>
  <div class="summary grid gap-3">
    <div class="tile border-stroke bg-card">
      <p class="text-xs text-muted">Source</p>
      <p class="value">{{ formatBytes(summary.left.size) }}</p>
      <p class="detail">
        {{ plural(summary.left.files, 'file') }} · {{ plural(summary.left.dirs, 'folder') }}
      </p>
    </div>
    <div class="tile border-stroke bg-card">
      <p class="text-xs text-muted">Destination</p>
      <p class="value">{{ formatBytes(summary.right.size) }}</p>
      <p class="detail">{{ plural(summary.right.files, 'file') }} · {{ deltaText }}</p>
    </div>
    <button
      type="button"
      class="tile text-left"
      :class="
        hasMissing
          ? 'border-danger/40 bg-danger-bg hover:border-danger/70'
          : 'border-stroke bg-card hover:bg-card-hover'
      "
      :aria-pressed="filter === 'missing'"
      title="Show only what is missing"
      data-testid="missing-tile"
      @click="emit('filter', 'missing')"
    >
      <p class="text-xs text-muted">Missing at destination</p>
      <p
        class="value"
        :class="hasMissing ? 'text-danger' : 'text-success'"
      >
        {{ hasMissing ? plural(summary.missing, 'file') : 'None' }}
      </p>
      <p class="detail">
        {{ hasMissing ? `${formatBytes(summary.missingBytes)} not copied` : 'Every file is there' }}
      </p>
    </button>
    <button
      type="button"
      class="tile border-stroke bg-card text-left hover:bg-card-hover"
      :aria-pressed="filter === 'changes'"
      title="Show every difference"
      @click="emit('filter', 'changes')"
    >
      <p class="text-xs text-muted">Other differences</p>
      <p
        class="value"
        :class="summary.different > 0 ? 'text-warning' : hasOther ? '' : 'text-success'"
      >
        {{ hasOther ? plural(summary.different, 'size change') : 'None' }}
      </p>
      <p class="detail">{{ plural(summary.extra, 'file') }} only at destination</p>
    </button>
  </div>
</template>

<style scoped>
.summary {
  grid-template-columns: repeat(2, minmax(0, 1fr));
}

/* Four across when the page is wide enough (container is the page column). */
@container page (min-width: 46rem) {
  .summary {
    grid-template-columns: repeat(4, minmax(0, 1fr));
  }
}

.tile {
  min-width: 0;
  border-width: 1px;
  border-radius: 0.5rem;
  padding: 0.625rem 1rem;
  transition:
    background-color 0.15s,
    border-color 0.15s;
}

.tile[aria-pressed='true'] {
  box-shadow: inset 0 -2px 0 var(--app-accent);
}

.value {
  margin-top: 0.125rem;
  font-family: var(--font-display);
  font-size: 1.25rem;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  line-height: 1.75rem;
  white-space: nowrap;
}

.detail {
  overflow: hidden;
  color: var(--app-muted);
  font-size: 0.75rem;
  font-variant-numeric: tabular-nums;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
