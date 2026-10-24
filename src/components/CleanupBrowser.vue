<script setup lang="ts">
import { File, Folder, Link2, TriangleAlert } from '@lucide/vue'
import TreemapView from '@/components/TreemapView.vue'
import { HARD_LINK_TITLE, type CleanupRow } from '@/lib/cleanup'
import { formatBytes, percentShare, plural } from '@/lib/format'

defineProps<{
  /** Rows as shown: `size` already follows the Size on disk / Size toggle. */
  rows: CleanupRow[]
  pickedId: number | null
  checked: Set<number>
  shownSize: number
}>()
const emit = defineEmits<{
  pick: [row: CleanupRow]
  open: [row: CleanupRow]
  menu: [row: CleanupRow, x: number, y: number]
  menuAtFocus: [row: CleanupRow, event: KeyboardEvent]
  toggle: [id: number]
}>()
</script>

<template>
  <div class="split mt-2 grid gap-3">
    <TreemapView
      class="h-80"
      :rows="rows"
      :selected="pickedId"
      @select="emit('pick', $event)"
      @open="emit('open', $event)"
      @menu="(row, x, y) => emit('menu', row, x, y)"
    />
    <ul
      class="h-80 overflow-y-auto rounded-lg border border-stroke bg-card"
      aria-label="Contents, largest first"
    >
      <li
        v-for="row in rows"
        :key="row.id"
        class="flex items-center"
      >
        <input
          type="checkbox"
          class="ml-3 shrink-0"
          :checked="checked.has(row.id)"
          :aria-label="`Select ${row.name}`"
          @change="emit('toggle', row.id)"
        />
        <button
          type="button"
          class="flex min-w-0 flex-1 items-center gap-2 px-3 py-1.5 text-left text-[13px] hover:bg-subtle"
          :class="{ 'bg-accent/12 hover:bg-accent/16': pickedId === row.id }"
          :title="row.error ? `${row.name}: ${row.error}` : row.name"
          @click="emit('pick', row)"
          @dblclick="emit('open', row)"
          @keydown.enter.prevent="emit('open', row)"
          @keydown.context-menu.prevent="emit('menuAtFocus', row, $event)"
          @keydown.shift.f10.prevent="emit('menuAtFocus', row, $event)"
          @contextmenu.prevent="emit('menu', row, $event.clientX, $event.clientY)"
        >
          <component
            :is="row.kind === 'dir' ? Folder : File"
            class="size-4 shrink-0"
            :style="{ color: row.kind === 'dir' ? 'var(--app-type-folder)' : undefined }"
            :stroke-width="1.75"
          />
          <span class="min-w-0 flex-1 truncate">{{ row.name }}</span>
          <Link2
            v-if="row.hardLinked"
            class="size-3.5 shrink-0 text-muted"
            :title="HARD_LINK_TITLE"
            aria-label="Hard link"
          />
          <TriangleAlert
            v-if="row.errors > 0"
            class="size-3.5 shrink-0 text-warning"
            :title="`${plural(row.errors, 'item')} inside could not be read`"
            :aria-label="`${plural(row.errors, 'item')} inside could not be read`"
          />
          <span class="w-11 shrink-0 text-right text-xs text-muted tabular-nums">
            {{ percentShare(row.size, shownSize) }}
          </span>
          <span class="w-20 shrink-0 text-right tabular-nums">{{ formatBytes(row.size) }}</span>
        </button>
      </li>
      <li
        v-if="rows.length === 0"
        class="px-3 py-6 text-center text-[13px] text-muted"
      >
        This folder is empty.
      </li>
    </ul>
  </div>
</template>

<style scoped>
.split {
  grid-template-columns: minmax(0, 1fr);
}

@container page (min-width: 46rem) {
  .split {
    grid-template-columns: minmax(0, 3fr) minmax(0, 2fr);
  }
}
</style>
