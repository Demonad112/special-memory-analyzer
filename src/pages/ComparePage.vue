<script setup lang="ts">
import { isTauri } from '@tauri-apps/api/core'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import {
  ArrowLeftRight,
  ArrowRight,
  ChevronsDownUp,
  ChevronsUpDown,
  CircleCheck,
  ClipboardList,
  Copy,
  FileText,
  FolderOpen,
  History,
  Info,
  LoaderCircle,
  TriangleAlert,
} from '@lucide/vue'
import {
  computed,
  nextTick,
  onBeforeUnmount,
  onMounted,
  reactive,
  ref,
  shallowRef,
  watch,
} from 'vue'
import { useRoute, useRouter } from 'vue-router'
import CompareTable from '@/components/CompareTable.vue'
import JobFields from '@/components/JobFields.vue'
import ContextMenu, { type MenuItem } from '@/components/ContextMenu.vue'
import PageHeader from '@/components/PageHeader.vue'
import PathField from '@/components/PathField.vue'
import SummaryStrip from '@/components/SummaryStrip.vue'
import { copyText } from '@/lib/clipboard'
import {
  cancelCompare,
  cleanPath,
  errorMessage,
  formatMissingList,
  ignoreJunk,
  isCancelled,
  loadChildren,
  loadMissing,
  loadPaths,
  loadRecent,
  missingDelta,
  pickFolder,
  rememberRecent,
  revealRow,
  rowPath,
  savePaths,
  startCompare,
  type ComparePaths,
  type CompareProgress,
  type CompareResult,
  type DiffRow,
  type Side,
} from '@/lib/compare'
import {
  defaultSort,
  nextSort,
  visibleRows,
  type CompareFilter,
  type Sort,
  type SortKey,
} from '@/lib/diffRows'
import { formatBytes, formatDuration, plural } from '@/lib/format'
import {
  copyMissingRecord,
  isCancelled as isRecordCancelled,
  recordJob,
  type RecordProgress,
} from '@/lib/record'
import { openReport } from '@/lib/reports'
import { sections } from '@/router'

const section = sections.find((candidate) => candidate.path === '/compare')
const paths = reactive(loadPaths())
watch(paths, () => savePaths(paths))
const recent = ref<ComparePaths[]>(loadRecent())

const running = ref(false)
const progress = ref<CompareProgress | null>(null)
const result = shallowRef<CompareResult | null>(null)
const compared = ref<ComparePaths>({ left: '', right: '' })
const finishedAt = ref<Date | null>(null)
const error = ref('')
const notice = ref('')

const children = shallowRef(new Map<number, DiffRow[]>())
const expanded = shallowRef(new Set<number>())
const loading = shallowRef(new Set<number>())
const filter = ref<CompareFilter>('all')
const sort = ref<Sort>(defaultSort)
const selected = ref<number | null>(null)

const canCompare = computed(
  () => !running.value && cleanPath(paths.left) !== '' && cleanPath(paths.right) !== '',
)
const rows = computed(() =>
  result.value
    ? visibleRows(result.value.rows, children.value, expanded.value, filter.value, sort.value)
    : [],
)
const unreadable = computed(() =>
  result.value ? result.value.summary.leftErrors + result.value.summary.rightErrors : 0,
)
const cloudFiles = computed(() =>
  result.value ? result.value.summary.leftCloud + result.value.summary.rightCloud : 0,
)

const filters: { value: CompareFilter; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'changes', label: 'Differences' },
  { value: 'missing', label: 'Missing' },
]

// Elapsed time while a compare runs.
const startedAt = ref(0)
const now = ref(0)
let ticker: ReturnType<typeof setInterval> | undefined
const elapsed = computed(() => {
  const seconds = Math.max(0, Math.floor((now.value - startedAt.value) / 1000))
  return `${String(Math.floor(seconds / 60))}:${String(seconds % 60).padStart(2, '0')}`
})

const resultsEl = ref<HTMLElement | null>(null)

async function runCompare(pair?: ComparePaths): Promise<void> {
  if (pair) {
    paths.left = pair.left
    paths.right = pair.right
  }
  if (!canCompare.value) {
    return
  }
  const left = cleanPath(paths.left)
  const right = cleanPath(paths.right)
  paths.left = left
  paths.right = right
  running.value = true
  confirmingCopy.value = false
  error.value = ''
  notice.value = ''
  progress.value = null
  result.value = null
  selected.value = null
  sort.value = defaultSort
  children.value = new Map()
  expanded.value = new Set()
  loading.value = new Set()
  startedAt.value = Date.now()
  now.value = startedAt.value
  ticker = setInterval(() => {
    now.value = Date.now()
  }, 250)
  try {
    result.value = await startCompare(left, right, ignoreJunk.value, (update) => {
      progress.value = update
    })
    compared.value = { left, right }
    finishedAt.value = new Date()
    filter.value = result.value.summary.missing > 0 ? 'missing' : 'all'
    notice.value = missingDelta({ left, right }, result.value.summary.missing) ?? ''
    recent.value = rememberRecent({ left, right })
    await nextTick()
    resultsEl.value?.scrollIntoView({ block: 'start', behavior: 'smooth' })
  } catch (err) {
    if (isCancelled(err)) {
      notice.value = 'Compare cancelled. Nothing was changed.'
    } else {
      error.value = errorMessage(err)
    }
  } finally {
    clearInterval(ticker)
    running.value = false
  }
}

async function cancel(): Promise<void> {
  await cancelCompare()
}

async function browse(side: Side): Promise<void> {
  const picked = await pickFolder(
    side === 'left' ? 'Choose the source folder' : 'Choose the destination folder',
    cleanPath(paths[side]),
  )
  if (picked) {
    paths[side] = picked
  }
}

function swap(): void {
  ;[paths.left, paths.right] = [paths.right, paths.left]
}

function setSort(key: Exclude<SortKey, 'size'>): void {
  sort.value = nextSort(sort.value, key)
}

async function toggle(id: number): Promise<void> {
  const next = new Set(expanded.value)
  if (next.has(id)) {
    next.delete(id)
    expanded.value = next
    return
  }
  next.add(id)
  expanded.value = next
  if (children.value.has(id) || loading.value.has(id)) {
    return
  }
  loading.value = new Set(loading.value).add(id)
  try {
    const loaded = await loadChildren(id)
    children.value = new Map(children.value).set(id, loaded)
  } catch (err) {
    showToast(errorMessage(err))
    const collapsed = new Set(expanded.value)
    collapsed.delete(id)
    expanded.value = collapsed
  } finally {
    const done = new Set(loading.value)
    done.delete(id)
    loading.value = done
  }
}

// Short confirmation or error at the bottom of the window.
const toast = ref('')
let toastTimer: ReturnType<typeof setTimeout> | undefined
function showToast(message: string): void {
  toast.value = message
  clearTimeout(toastTimer)
  toastTimer = setTimeout(() => {
    toast.value = ''
  }, 3500)
}

async function reveal(row: DiffRow, side: Side): Promise<void> {
  try {
    await revealRow(row.id, side)
  } catch (err) {
    showToast(errorMessage(err))
  }
}

function openRow(row: DiffRow): void {
  void reveal(row, row.left ? 'left' : 'right')
}

async function copyPath(row: DiffRow, side: Side): Promise<void> {
  try {
    await copyText(await rowPath(row.id, side))
    showToast('Path copied')
  } catch (err) {
    showToast(errorMessage(err))
  }
}

async function openCompareReport(): Promise<void> {
  if (!result.value?.reportId) {
    return
  }
  try {
    await openReport({ kind: 'compare', id: result.value.reportId })
  } catch (err) {
    showToast(errorMessage(err))
  }
}

const confirmingCopy = ref(false)

/** Copies the missing files as a Record; shows its live panel, then opens its report. */
async function copyMissingFiles(): Promise<void> {
  confirmingCopy.value = false
  if (recordJob.running) {
    showToast('A record is already running. Finish or cancel it first.')
    return
  }
  recordJob.running = true
  recordJob.mode = 'copy'
  recordJob.progress = null
  recordJob.copyStartedAt = 0
  recordJob.finishing = false
  await router.push('/compare/record')
  try {
    const details = await copyMissingRecord('sizeAndTime', (update: RecordProgress) => {
      if (update.phase === 'done') {
        return
      }
      if (update.phase !== 'scanning' && recordJob.copyStartedAt === 0) {
        recordJob.copyStartedAt = Date.now()
      }
      recordJob.progress = update
    })
    if (route.path.startsWith('/compare')) {
      await router.push(`/compare/record/${details.summary.id}`)
    }
  } catch (err) {
    if (!isRecordCancelled(err)) {
      showToast(errorMessage(err))
    }
  } finally {
    recordJob.running = false
  }
}

async function copyMissing(): Promise<void> {
  if (!result.value) {
    return
  }
  try {
    const list = await loadMissing()
    await copyText(formatMissingList(list, result.value.summary, finishedAt.value ?? new Date()))
    showToast(`Copied ${plural(list.paths.length, 'path')}. Paste it into an email or ticket.`)
  } catch (err) {
    showToast(errorMessage(err))
  }
}

const menu = shallowRef<{ row: DiffRow; x: number; y: number } | null>(null)
const menuItems = computed<(MenuItem | null)[]>(() => {
  const row = menu.value?.row
  if (!row) {
    return []
  }
  const items: (MenuItem | null)[] = [
    {
      label: 'Show source in Explorer',
      icon: FolderOpen,
      disabled: !row.left,
      action: () => void reveal(row, 'left'),
    },
    {
      label: 'Show destination in Explorer',
      icon: FolderOpen,
      disabled: !row.right,
      action: () => void reveal(row, 'right'),
    },
    null,
    {
      label: 'Copy source path',
      icon: Copy,
      disabled: !row.left,
      action: () => void copyPath(row, 'left'),
    },
    {
      label: 'Copy destination path',
      icon: Copy,
      disabled: !row.right,
      action: () => void copyPath(row, 'right'),
    },
  ]
  if (row.hasChildren) {
    const isOpen = expanded.value.has(row.id)
    items.push(null, {
      label: isOpen ? 'Collapse' : 'Expand',
      icon: isOpen ? ChevronsDownUp : ChevronsUpDown,
      action: () => void toggle(row.id),
    })
  }
  return items
})

function closeMenu(): void {
  menu.value = null
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape' && running.value) {
    void cancel()
  }
}

// Dropping a folder from Explorer onto either field fills it; dropping elsewhere fills the first
// empty field.
const fieldRefs = reactive<Record<Side, HTMLElement | null>>({ left: null, right: null })
const dropSide = ref<Side | null>(null)
let stopDragDrop: (() => void) | undefined

function sideAt(x: number, y: number): Side | null {
  for (const side of ['left', 'right'] as const) {
    const box = fieldRefs[side]?.getBoundingClientRect()
    if (box && x >= box.left && x <= box.right && y >= box.top && y <= box.bottom) {
      return side
    }
  }
  return null
}

const route = useRoute()
const router = useRouter()

onMounted(async () => {
  window.addEventListener('keydown', onKeydown)
  // Home's "Recent compares" opens this page with ?run=1 after filling the paths.
  if (route.query.run) {
    void router.replace({ query: {} })
    void runCompare()
  }
  if (!isTauri()) {
    return
  }
  stopDragDrop = await getCurrentWebview().onDragDropEvent(({ payload }) => {
    if (payload.type === 'leave') {
      dropSide.value = null
      return
    }
    const scale = window.devicePixelRatio || 1
    const side = sideAt(payload.position.x / scale, payload.position.y / scale)
    if (payload.type === 'drop') {
      dropSide.value = null
      const [path] = payload.paths
      const target = side ?? (paths.left ? 'right' : 'left')
      if (path && !running.value) {
        paths[target] = path
      }
      return
    }
    dropSide.value = side
  })
})
onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown)
  stopDragDrop?.()
  clearInterval(ticker)
  clearTimeout(toastTimer)
})

function shortPath(path: string): string {
  return path.length > 48 ? `…${path.slice(-47)}` : path
}
</script>

<template>
  <div class="page mx-auto flex min-h-full max-w-6xl flex-col px-6 py-6 lg:px-10 lg:py-8">
    <div class="flex items-start justify-between gap-6">
      <PageHeader
        :title="section?.title ?? 'Compare'"
        summary="Compare two folders by size and find every file that is missing at the destination."
      />
      <div
        v-if="!running"
        class="mt-1.5 flex shrink-0 gap-2"
      >
        <RouterLink
          to="/compare/record"
          class="inline-flex h-8 items-center rounded-md border border-stroke bg-card px-4 hover:bg-card-hover"
          title="Copy the source to the destination, or watch another program copy it, and list every file that did not make it"
        >
          Record copy
        </RouterLink>
        <button
          type="button"
          class="h-8 rounded-md bg-accent px-5 font-semibold text-on-accent hover:bg-accent-hover disabled:opacity-50"
          :disabled="!canCompare"
          @click="runCompare()"
        >
          Compare
        </button>
      </div>
      <button
        v-else
        type="button"
        class="mt-1.5 h-8 shrink-0 rounded-md border border-stroke bg-card px-5 hover:bg-card-hover"
        title="Cancel (Esc)"
        @click="cancel"
      >
        Cancel
      </button>
    </div>

    <div class="grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-center gap-2">
      <div :ref="(el) => (fieldRefs.left = el as HTMLElement | null)">
        <PathField
          v-model="paths.left"
          label="Source"
          hint="Copied from"
          :disabled="running"
          :drop-target="dropSide === 'left'"
          @browse="browse('left')"
          @submit="runCompare()"
        />
      </div>
      <button
        type="button"
        class="grid size-8 place-items-center rounded-md text-muted hover:bg-subtle-strong disabled:opacity-50"
        title="Swap source and destination"
        aria-label="Swap source and destination"
        :disabled="running"
        @click="swap"
      >
        <ArrowLeftRight class="size-4" />
      </button>
      <div :ref="(el) => (fieldRefs.right = el as HTMLElement | null)">
        <PathField
          v-model="paths.right"
          label="Destination"
          hint="Copied to"
          :disabled="running"
          :drop-target="dropSide === 'right'"
          @browse="browse('right')"
          @submit="runCompare()"
        />
      </div>
    </div>
    <label class="mt-2 flex w-fit items-center gap-2 text-[13px] text-muted">
      <input
        v-model="ignoreJunk"
        type="checkbox"
        class="size-4 accent-accent"
        :disabled="running"
      />
      Ignore system and temp files (Thumbs.db, desktop.ini, ~$ Office lock files, .tmp)
    </label>
    <JobFields
      class="mt-3"
      :disabled="running"
    />

    <p
      v-if="error"
      class="mt-4 flex items-start gap-2 rounded-lg border border-danger/40 bg-danger-bg px-4 py-3 text-[13px] text-danger"
      role="alert"
    >
      <TriangleAlert class="mt-0.5 size-4 shrink-0" />
      {{ error }}
    </p>
    <p
      v-if="notice"
      class="mt-4 flex items-center gap-2 text-[13px] text-muted"
      role="status"
    >
      <Info class="size-4 shrink-0" />
      {{ notice }}
    </p>

    <section
      v-if="running"
      class="mt-6 rounded-lg border border-stroke bg-card px-5 py-4"
      aria-live="polite"
    >
      <div class="flex items-center justify-between gap-4">
        <p class="flex items-center gap-2 font-semibold">
          <LoaderCircle class="size-4 animate-spin text-accent" />
          Reading both folders…
        </p>
        <p class="text-xs text-muted tabular-nums">{{ elapsed }} · Esc to cancel</p>
      </div>
      <div class="indeterminate mt-3 h-1 overflow-hidden rounded-full bg-subtle-strong" />
      <div class="mt-3 grid grid-cols-2 gap-6">
        <div
          v-for="side in ['left', 'right'] as const"
          :key="side"
          class="min-w-0"
        >
          <p class="flex items-center gap-1.5 text-xs text-muted">
            {{ side === 'left' ? 'Source' : 'Destination' }}
            <span
              v-if="progress?.[side === 'left' ? 'leftDone' : 'rightDone']"
              class="flex items-center gap-1 text-success"
            >
              <CircleCheck class="size-3.5" />
              Done
            </span>
          </p>
          <p class="tabular-nums">
            {{ plural(progress?.[side].files ?? 0, 'file') }} ·
            {{ formatBytes(progress?.[side].bytes ?? 0) }}
          </p>
          <p
            class="truncate text-xs text-faint"
            :title="progress?.[side].current"
          >
            {{ progress?.[side].current || ' ' }}
          </p>
        </div>
      </div>
    </section>

    <div
      v-else-if="result"
      ref="resultsEl"
      class="flex flex-1 scroll-mt-4 flex-col pt-5"
    >
      <SummaryStrip
        :summary="result.summary"
        :filter="filter"
        @filter="filter = $event"
      />
      <div
        v-if="unreadable > 0 || cloudFiles > 0"
        class="mt-3 flex flex-wrap gap-x-6 gap-y-1 text-[13px]"
      >
        <p
          v-if="unreadable > 0"
          class="flex items-center gap-2 text-warning"
        >
          <TriangleAlert class="size-4 shrink-0" />
          {{ plural(unreadable, 'folder') }} not fully readable (marked below); sizes there may be
          low.
        </p>
        <p
          v-if="cloudFiles > 0"
          class="flex items-center gap-2 text-muted"
        >
          <Info class="size-4 shrink-0" />
          {{ plural(cloudFiles, 'online-only cloud file') }}: size compared, nothing downloaded.
        </p>
      </div>

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
              filter === option.value
                ? 'bg-accent text-on-accent'
                : 'text-fg hover:bg-subtle-strong'
            "
            @click="filter = option.value"
          >
            {{ option.label }}
          </button>
        </div>
        <p
          class="min-w-0 flex-1 truncate text-right text-xs text-muted tabular-nums"
          :title="`${compared.left} → ${compared.right}`"
        >
          Compared
          {{ finishedAt?.toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' }) }}
          in {{ formatDuration(result.elapsedMs) }}
        </p>
        <button
          v-if="result.reportId"
          type="button"
          class="flex h-8 shrink-0 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover"
          title="Open the saved report in the browser (print or save as PDF from there). Export it from Reports."
          @click="openCompareReport"
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
          @click="copyMissing"
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
          class="flex h-8 shrink-0 items-center gap-1.5 rounded-md bg-accent px-3 text-[13px] text-white hover:opacity-90"
          title="Copy the missing files to the destination as a record, with progress, a report and retry"
          @click="confirmingCopy = true"
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
          @click="runCompare(compared)"
        >
          <History
            class="size-4"
            :stroke-width="1.75"
          />
          Re-check
        </button>
      </div>
      <div
        v-if="confirmingCopy"
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
          class="h-8 rounded-md bg-accent px-3 text-white hover:opacity-90"
          @click="copyMissingFiles"
        >
          Copy
        </button>
        <button
          type="button"
          class="h-8 rounded-md border border-stroke px-3 hover:bg-card-hover"
          @click="confirmingCopy = false"
        >
          Cancel
        </button>
      </div>

      <CompareTable
        v-if="rows.length > 0"
        class="mt-3 min-h-72 flex-1"
        :rows="rows"
        :loading="loading"
        :sort="sort"
        :selected="selected"
        @toggle="toggle"
        @select="selected = $event"
        @open="openRow"
        @menu="(row, x, y) => (menu = { row, x, y })"
        @sort="setSort"
      />
      <div
        v-else
        class="mt-3 grid min-h-40 flex-1 place-items-center rounded-lg border border-stroke bg-card text-center"
      >
        <div>
          <CircleCheck
            class="mx-auto size-8 text-success"
            :stroke-width="1.5"
          />
          <p class="mt-2 font-semibold">
            {{
              filter === 'missing'
                ? 'Nothing is missing'
                : filter === 'changes'
                  ? 'No differences'
                  : 'Both folders are empty'
            }}
          </p>
          <p class="mt-1 text-[13px] text-muted">
            {{
              filter === 'missing'
                ? 'Every source file is at the destination.'
                : filter === 'changes'
                  ? 'Both folders hold the same files with the same sizes.'
                  : 'There is nothing to compare.'
            }}
          </p>
        </div>
      </div>
      <p
        v-if="rows.length > 0"
        class="mt-2 text-xs text-faint"
      >
        Double-click a file to show it in Explorer. Right-click for more. Arrow keys move through
        the list.
      </p>
    </div>

    <div
      v-else-if="!error"
      class="mt-6 grid flex-1 place-items-center rounded-lg border border-dashed border-stroke-strong px-6 py-10 text-center"
    >
      <div class="w-full max-w-lg">
        <component
          :is="section?.icon"
          class="mx-auto size-10 text-faint"
          :stroke-width="1.25"
        />
        <p class="mt-3 font-semibold">Pick two folders</p>
        <p class="mx-auto mt-1 max-w-md text-[13px] text-muted">
          Type, paste, browse or drag a folder from Explorer into each box. Folder sizes include
          everything inside them, and files missing at the destination show in red.
        </p>
        <div
          v-if="recent.length > 0"
          class="mt-6 text-left"
        >
          <p class="mb-1.5 flex items-center gap-1.5 text-xs font-semibold text-muted">
            <History class="size-3.5" />
            Recent
          </p>
          <ul class="overflow-hidden rounded-lg border border-stroke bg-card">
            <li
              v-for="pair in recent"
              :key="`${pair.left}|${pair.right}`"
              class="border-b border-stroke last:border-b-0"
            >
              <button
                type="button"
                class="flex w-full items-center gap-2 px-3 py-2 text-left text-[13px] hover:bg-subtle"
                :title="`Compare ${pair.left} with ${pair.right}`"
                @click="runCompare(pair)"
              >
                <span class="min-w-0 flex-1 truncate">{{ shortPath(pair.left) }}</span>
                <ArrowRight class="size-3.5 shrink-0 text-faint" />
                <span class="min-w-0 flex-1 truncate">{{ shortPath(pair.right) }}</span>
              </button>
            </li>
          </ul>
        </div>
      </div>
    </div>

    <ContextMenu
      v-if="menu"
      :items="menuItems"
      :x="menu.x"
      :y="menu.y"
      @close="closeMenu"
    />
    <Transition name="toast">
      <p
        v-if="toast"
        class="fixed bottom-5 left-1/2 z-40 max-w-[min(32rem,90vw)] -translate-x-1/2 rounded-md bg-fg px-4 py-2 text-[13px] text-app shadow-lg"
        role="status"
      >
        {{ toast }}
      </p>
    </Transition>
  </div>
</template>

<style scoped>
.page {
  container: page / inline-size;
}

/* Fluent indeterminate progress: a short accent bar sliding across. */
.indeterminate {
  position: relative;
}

.indeterminate::after {
  position: absolute;
  inset: 0 auto 0 0;
  width: 30%;
  border-radius: 999px;
  background: var(--app-accent);
  animation: slide 1.4s ease-in-out infinite;
  content: '';
}

@keyframes slide {
  from {
    transform: translateX(-100%);
  }

  to {
    transform: translateX(340%);
  }
}

.toast-enter-active,
.toast-leave-active {
  transition:
    opacity 0.15s,
    transform 0.15s;
}

.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translate(-50%, 6px);
}

@media (prefers-reduced-motion: reduce) {
  .indeterminate::after {
    animation-duration: 4s;
  }
}
</style>
