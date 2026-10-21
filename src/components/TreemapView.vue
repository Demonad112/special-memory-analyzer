<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import type { CleanupRow } from '@/lib/cleanup'
import { categoryOf } from '@/lib/fileTypes'
import { formatBytes } from '@/lib/format'
import { squarify } from '@/lib/treemap'

const props = defineProps<{ rows: CleanupRow[]; selected: number | null }>()
const emit = defineEmits<{
  select: [row: CleanupRow]
  open: [row: CleanupRow]
  menu: [row: CleanupRow, x: number, y: number]
}>()

/** Past this many tiles the rest share one grey tile; they would be too small to read anyway. */
const MAX_TILES = 300
/** Gap between tiles, in px. */
const GAP = 2

const box = ref<HTMLElement | null>(null)
const size = ref({ width: 0, height: 0 })
let observer: ResizeObserver | undefined
onMounted(() => {
  observer = new ResizeObserver(([entry]) => {
    if (entry) {
      size.value = { width: entry.contentRect.width, height: entry.contentRect.height }
    }
  })
  if (box.value) {
    observer.observe(box.value)
  }
})
onBeforeUnmount(() => observer?.disconnect())

interface Item {
  row: CleanupRow | null
  name: string
  size: number
}

const tiles = computed(() => {
  const shown: Item[] = props.rows
    .slice(0, MAX_TILES)
    .map((row) => ({ row, name: row.name, size: row.size }))
  const rest = props.rows.slice(MAX_TILES)
  const restSize = rest.reduce((sum, row) => sum + row.size, 0)
  if (restSize > 0) {
    shown.push({ row: null, name: `${String(rest.length)} smaller items`, size: restSize })
  }
  return squarify(shown, { x: 0, y: 0, ...size.value }).map((tile) => ({
    ...tile,
    category: tile.item.row ? categoryOf(tile.item.row.name, tile.item.row.kind) : 'other',
    labelled: tile.width >= 64 && tile.height >= 30,
  }))
})
</script>

<template>
  <div
    ref="box"
    class="relative overflow-hidden rounded-lg border border-stroke bg-card"
  >
    <button
      v-for="tile in tiles"
      :key="tile.item.row?.id ?? -1"
      type="button"
      class="tile absolute overflow-hidden rounded-[3px] px-1.5 py-1 text-left text-[11px] leading-tight"
      :class="{ selected: tile.item.row && tile.item.row.id === selected, rest: !tile.item.row }"
      :style="{
        left: `${String(tile.x + GAP / 2)}px`,
        top: `${String(tile.y + GAP / 2)}px`,
        width: `${String(Math.max(0, tile.width - GAP))}px`,
        height: `${String(Math.max(0, tile.height - GAP))}px`,
        background: `var(--app-type-${tile.category})`,
      }"
      :title="`${tile.item.name}\n${formatBytes(tile.item.size)}`"
      :disabled="!tile.item.row"
      tabindex="-1"
      @click="tile.item.row && emit('select', tile.item.row)"
      @dblclick="tile.item.row && emit('open', tile.item.row)"
      @contextmenu.prevent="
        tile.item.row && emit('menu', tile.item.row, $event.clientX, $event.clientY)
      "
    >
      <template v-if="tile.labelled">
        <span class="block truncate font-semibold">{{ tile.item.name }}</span>
        <span class="block truncate tabular-nums opacity-85">
          {{ formatBytes(tile.item.size) }}
        </span>
      </template>
    </button>
    <p
      v-if="tiles.length === 0"
      class="grid h-full place-items-center text-[13px] text-muted"
    >
      Nothing here takes up space.
    </p>
  </div>
</template>

<style scoped>
.tile {
  color: var(--app-type-text);
  transition: filter 0.1s;
}

.tile:hover:not(:disabled) {
  filter: brightness(1.12);
}

.tile.rest {
  opacity: 0.45;
}

.tile.selected {
  outline: 2px solid var(--app-text);
  outline-offset: -2px;
  box-shadow: inset 0 0 0 3px var(--app-bg);
}
</style>
