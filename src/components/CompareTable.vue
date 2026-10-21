<script setup lang="ts">
import {
  ArrowDown,
  ArrowUp,
  ChevronRight,
  Cloud,
  File,
  Folder,
  Link2,
  LoaderCircle,
  TriangleAlert,
} from '@lucide/vue'
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import StatusChip from '@/components/StatusChip.vue'
import type { DiffRow, DiffSide } from '@/lib/compare'
import { sizeDelta, statusLabel, type Sort, type SortKey, type VisibleRow } from '@/lib/diffRows'
import { formatBytes, formatDelta } from '@/lib/format'

const props = defineProps<{
  rows: VisibleRow[]
  loading: ReadonlySet<number>
  sort: Sort
  selected: number | null
}>()
const emit = defineEmits<{
  toggle: [id: number]
  select: [id: number]
  /** Enter or double-click on a file. */
  open: [row: DiffRow]
  menu: [row: DiffRow, x: number, y: number]
  sort: [key: Exclude<SortKey, 'size'>]
}>()

/** Fixed row height so only the rows on screen are rendered (a fully expanded tree can be huge). */
const rowHeight = 32
const overscan = 12

const viewport = ref<HTMLElement | null>(null)
const scrollTop = ref(0)
const viewportHeight = ref(480)
let observer: ResizeObserver | undefined

onMounted(() => {
  if (viewport.value && typeof ResizeObserver !== 'undefined') {
    observer = new ResizeObserver(([entry]) => {
      if (entry) {
        viewportHeight.value = entry.contentRect.height
      }
    })
    observer.observe(viewport.value)
  }
})
onBeforeUnmount(() => observer?.disconnect())

// A filter change can shrink the list below the current scroll position.
watch(
  () => props.rows.length,
  (length) => {
    const max = Math.max(0, length * rowHeight - viewportHeight.value)
    if (viewport.value && scrollTop.value > max) {
      viewport.value.scrollTop = max
      scrollTop.value = max
    }
  },
)

const first = computed(() => Math.max(0, Math.floor(scrollTop.value / rowHeight) - overscan))
const last = computed(() =>
  Math.min(
    props.rows.length,
    Math.ceil((scrollTop.value + viewportHeight.value) / rowHeight) + overscan,
  ),
)
const shown = computed(() => props.rows.slice(first.value, last.value))
const selectedIndex = computed(() =>
  props.rows.findIndex((visible) => visible.row.id === props.selected),
)

function onScroll(event: Event): void {
  scrollTop.value = (event.target as HTMLElement).scrollTop
}

function scrollToIndex(index: number): void {
  const element = viewport.value
  if (!element) {
    return
  }
  const top = index * rowHeight
  if (top < element.scrollTop) {
    element.scrollTop = top
  } else if (top + rowHeight > element.scrollTop + element.clientHeight) {
    element.scrollTop = top + rowHeight - element.clientHeight
  }
}

function selectIndex(index: number): void {
  const clamped = Math.max(0, Math.min(props.rows.length - 1, index))
  const target = props.rows[clamped]
  if (target) {
    emit('select', target.row.id)
    scrollToIndex(clamped)
  }
}

function openMenuForSelected(): void {
  const index = selectedIndex.value
  const visible = props.rows[index]
  const element = viewport.value
  if (visible && element) {
    const box = element.getBoundingClientRect()
    const top = box.top + index * rowHeight - element.scrollTop
    emit('menu', visible.row, box.left + 48, top + rowHeight)
  }
}

/** Arrow keys as in Explorer's tree: up/down move, right expands, left collapses or goes up. */
function onKeydown(event: KeyboardEvent): void {
  const index = selectedIndex.value
  const current = props.rows[index]
  const page = Math.max(1, Math.floor(viewportHeight.value / rowHeight) - 1)
  switch (event.key) {
    case 'ArrowDown':
      selectIndex(index + 1)
      break
    case 'ArrowUp':
      selectIndex(index < 0 ? 0 : index - 1)
      break
    case 'PageDown':
      selectIndex(index + page)
      break
    case 'PageUp':
      selectIndex(index - page)
      break
    case 'Home':
      selectIndex(0)
      break
    case 'End':
      selectIndex(props.rows.length - 1)
      break
    case 'ArrowRight':
      if (current?.row.hasChildren) {
        if (current.expanded) {
          selectIndex(index + 1)
        } else {
          emit('toggle', current.row.id)
        }
      }
      break
    case 'ArrowLeft':
      if (current?.expanded) {
        emit('toggle', current.row.id)
      } else if (current && current.parentId !== null) {
        const parentId = current.parentId
        selectIndex(props.rows.findIndex((visible) => visible.row.id === parentId))
      }
      break
    case 'Enter':
      if (current) {
        if (current.row.hasChildren) {
          emit('toggle', current.row.id)
        } else {
          emit('open', current.row)
        }
      }
      break
    case 'ContextMenu':
      openMenuForSelected()
      break
    case 'F10':
      if (!event.shiftKey) {
        return
      }
      openMenuForSelected()
      break
    default:
      return
  }
  event.preventDefault()
}

function onFocus(): void {
  if (selectedIndex.value < 0 && props.rows.length > 0) {
    selectIndex(0)
  }
}

function onDoubleClick(visible: VisibleRow): void {
  if (visible.row.hasChildren) {
    emit('toggle', visible.row.id)
  } else {
    emit('open', visible.row)
  }
}

function onContextMenu(event: MouseEvent, visible: VisibleRow): void {
  emit('select', visible.row.id)
  emit('menu', visible.row, event.clientX, event.clientY)
}

function size(side: DiffSide | null): string {
  return side ? formatBytes(side.size) : '—'
}

function delta(visible: VisibleRow): string {
  const bytes = sizeDelta(visible.row)
  return bytes === 0 ? '—' : formatDelta(bytes)
}

const headers: { key: Exclude<SortKey, 'size'>; label: string; class: string }[] = [
  { key: 'name', label: 'Name', class: 'justify-start' },
  { key: 'left', label: 'Source', class: 'justify-end' },
  { key: 'right', label: 'Destination', class: 'justify-end' },
  { key: 'delta', label: 'Difference', class: 'justify-end delta-col' },
]

const ariaSort = (key: SortKey): 'ascending' | 'descending' | 'none' =>
  props.sort.key === key ? (props.sort.dir === 'asc' ? 'ascending' : 'descending') : 'none'
</script>

<template>
  <div
    class="compare-table flex min-h-0 flex-col overflow-hidden rounded-lg border border-stroke bg-card"
    role="treegrid"
    aria-label="Compare result"
    :aria-rowcount="rows.length"
  >
    <div
      class="grid-row h-9 shrink-0 border-b border-stroke text-xs font-semibold text-muted"
      role="row"
    >
      <button
        v-for="header in headers"
        :key="header.key"
        type="button"
        role="columnheader"
        :aria-sort="ariaSort(header.key)"
        class="flex h-full items-center gap-1 hover:text-fg"
        :class="[header.class, sort.key === header.key ? 'text-fg' : '']"
        :title="`Sort by ${header.label.toLowerCase()}`"
        @click="emit('sort', header.key)"
      >
        {{ header.label }}
        <component
          :is="sort.dir === 'asc' ? ArrowUp : ArrowDown"
          v-if="sort.key === header.key"
          class="size-3"
        />
      </button>
      <span
        role="columnheader"
        class="flex items-center"
        >Status</span
      >
    </div>
    <div
      ref="viewport"
      class="rows min-h-0 flex-1 overflow-auto outline-none"
      tabindex="0"
      :aria-activedescendant="selected === null ? undefined : `compare-row-${String(selected)}`"
      @scroll.passive="onScroll"
      @keydown="onKeydown"
      @focus="onFocus"
    >
      <div
        class="relative"
        :style="{ height: `${String(rows.length * rowHeight)}px` }"
      >
        <div
          class="absolute inset-x-0"
          :style="{ top: `${String(first * rowHeight)}px` }"
        >
          <div
            v-for="visible in shown"
            :id="`compare-row-${String(visible.row.id)}`"
            :key="visible.row.id"
            class="grid-row text-[13px]"
            :class="[
              visible.row.status === 'onlyLeft' ? 'bg-danger-bg/60 text-danger' : 'hover:bg-subtle',
              visible.row.id === selected ? 'is-selected' : '',
            ]"
            :style="{ height: `${String(rowHeight)}px` }"
            role="row"
            :aria-level="visible.depth + 1"
            :aria-expanded="visible.row.hasChildren ? visible.expanded : undefined"
            :aria-selected="visible.row.id === selected"
            :data-status="visible.row.status"
            @click="emit('select', visible.row.id)"
            @dblclick="onDoubleClick(visible)"
            @contextmenu.prevent="onContextMenu($event, visible)"
          >
            <span
              class="flex min-w-0 items-center gap-1.5"
              :style="{ paddingLeft: `${String(visible.depth * 20)}px` }"
              role="gridcell"
            >
              <button
                v-if="visible.row.hasChildren"
                type="button"
                tabindex="-1"
                class="grid size-5 shrink-0 place-items-center rounded text-muted hover:bg-subtle-strong"
                :aria-label="visible.expanded ? 'Collapse' : 'Expand'"
                @click.stop="emit('toggle', visible.row.id)"
                @dblclick.stop
              >
                <LoaderCircle
                  v-if="loading.has(visible.row.id)"
                  class="size-3.5 animate-spin"
                />
                <ChevronRight
                  v-else
                  class="size-3.5 transition-transform"
                  :class="{ 'rotate-90': visible.expanded }"
                />
              </button>
              <span
                v-else
                class="w-5 shrink-0"
              />
              <component
                :is="
                  visible.row.kind === 'dir' ? Folder : visible.row.kind === 'link' ? Link2 : File
                "
                class="size-4 shrink-0"
                :class="visible.row.status === 'onlyLeft' ? '' : 'text-muted'"
                :stroke-width="1.75"
              />
              <span
                class="truncate"
                :title="visible.row.name"
                >{{ visible.row.name }}</span
              >
              <span
                v-if="visible.row.cloud"
                class="shrink-0 text-faint"
                title="Includes online-only cloud files: size shown, data not on this PC"
              >
                <Cloud
                  class="size-3.5"
                  :stroke-width="1.75"
                />
              </span>
              <span
                v-if="visible.row.error"
                class="shrink-0 text-warning"
                :title="`Not fully readable: ${visible.row.error}`"
              >
                <TriangleAlert
                  class="size-3.5"
                  :stroke-width="1.75"
                />
              </span>
            </span>
            <span
              class="text-right tabular-nums"
              role="gridcell"
              >{{ size(visible.row.left) }}</span
            >
            <span
              class="text-right tabular-nums"
              :class="visible.row.right ? '' : 'text-faint'"
              role="gridcell"
              >{{ size(visible.row.right) }}</span
            >
            <span
              class="delta-col text-right tabular-nums"
              :class="visible.row.status === 'onlyLeft' ? '' : 'text-muted'"
              role="gridcell"
              >{{ delta(visible) }}</span
            >
            <span
              class="min-w-0"
              role="gridcell"
            >
              <StatusChip v-bind="statusLabel(visible.row)" />
            </span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.compare-table {
  container-type: inline-size;
}

/* Name takes the rest; the Difference column drops out when the window is narrow. */
.grid-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 6rem 6.5rem 8.5rem;
  align-items: center;
  gap: 0.75rem;
  padding-inline: 0.75rem;
}

.delta-col {
  display: none;
}

@container (min-width: 42rem) {
  .grid-row {
    grid-template-columns: minmax(0, 1fr) 6.5rem 6.5rem 6.5rem 9rem;
  }

  button.delta-col {
    display: flex;
  }

  span.delta-col {
    display: block;
  }
}

.is-selected {
  background: var(--app-subtle-strong);
  box-shadow: inset 2px 0 0 var(--app-accent);
}

.rows:focus-visible .is-selected {
  outline: 1px solid var(--app-accent);
  outline-offset: -1px;
}
</style>
