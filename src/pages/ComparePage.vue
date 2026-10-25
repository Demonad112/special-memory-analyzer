<script setup lang="ts">
import { isTauri } from '@tauri-apps/api/core'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import {
  ArrowLeftRight,
  ArrowRight,
  ChevronsDownUp,
  ChevronsUpDown,
  CircleCheck,
  Copy,
  FolderOpen,
  History,
  Info,
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
  toRef,
  watch,
} from 'vue'
import { useRoute, useRouter } from 'vue-router'
import CompareProgress from '@/components/CompareProgress.vue'
import CompareTable from '@/components/CompareTable.vue'
import CompareToolbar from '@/components/CompareToolbar.vue'
import JobFields from '@/components/JobFields.vue'
import ContextMenu, { type MenuItem } from '@/components/ContextMenu.vue'
import PageHeader from '@/components/PageHeader.vue'
import PermissionDetailPanel from '@/components/PermissionDetailPanel.vue'
import PathField from '@/components/PathField.vue'
import ShareSignIn from '@/components/ShareSignIn.vue'
import { needsSignIn } from '@/lib/servers'
import SummaryStrip from '@/components/SummaryStrip.vue'
import { copyText } from '@/lib/clipboard'
import { errorMessage } from '@/lib/errors'
import { currentJob } from '@/lib/job'
import { useElapsed } from '@/lib/useElapsed'
import { useToast } from '@/lib/useToast'
import {
  cancelCompare,
  cleanPath,
  formatMissingList,
  ignoreJunk,
  loadMissing,
  loadPermissionDetail,
  loadPaths,
  loadRecent,
  pickFolder,
  revealRow,
  rowPath,
  savePaths,
  compareJob,
  runCompareJob,
  toggleRow,
  type ComparePaths,
  type DiffRow,
  type PermissionDetail,
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
import { plural } from '@/lib/format'
import {
  copyMissingRecord,
  isCancelled as isRecordCancelled,
  recordJob,
  type RecordProgress,
} from '@/lib/record'
import { exportReport, openReport } from '@/lib/reports'
import { sections } from '@/router'

const section = sections.find((candidate) => candidate.path === '/compare')
const paths = reactive(loadPaths())
watch(paths, () => savePaths(paths))
const recent = ref<ComparePaths[]>(loadRecent())

// The compare lives in `compareJob`, so it survives leaving this page.
const running = toRef(compareJob, 'running')
const progress = toRef(compareJob, 'progress')
const result = toRef(compareJob, 'result')
const compared = toRef(compareJob, 'compared')
const finishedAt = toRef(compareJob, 'finishedAt')
const error = toRef(compareJob, 'error')
const notice = toRef(compareJob, 'notice')

const children = toRef(compareJob, 'children')
const expanded = toRef(compareJob, 'expanded')
const loading = toRef(compareJob, 'loading')
const filter = ref<CompareFilter>('all')
const sort = ref<Sort>(defaultSort)
const selected = ref<number | null>(null)
/** "Compare permissions": off until asked for, because it reads every folder's security twice. */
const comparePermissions = ref(false)
const permissionsProgress = toRef(compareJob, 'permissionsProgress')

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

const elapsed = useElapsed(
  () => compareJob.startedAt,
  () => compareJob.running,
)

const resultsEl = ref<HTMLElement | null>(null)

/** Bumped when a new compare starts or the selection moves, so late answers write nothing. */
let resultSeq = 0
let detailSeq = 0
const detail = ref<PermissionDetail | null>(null)
const detailLoading = ref(false)
const exportingPermissions = ref(false)

const permissionsChecked = computed(() => result.value?.permissions?.status === 'checked')
const selectedRow = computed(() => rows.value.find((visible) => visible.row.id === selected.value))
const selectedCell = computed(() => selectedRow.value?.row.permissions)

watch(selected, async (id) => {
  const mine = ++detailSeq
  detail.value = null
  const cell = selectedCell.value
  if (id === null || !cell || cell.status === null || cell.status === 'same') {
    detailLoading.value = false
    return
  }
  detailLoading.value = true
  try {
    const loaded = await loadPermissionDetail(id)
    if (mine === detailSeq) {
      detail.value = loaded
    }
  } catch (err) {
    if (mine === detailSeq) {
      showToast(errorMessage(err))
    }
  } finally {
    if (mine === detailSeq) {
      detailLoading.value = false
    }
  }
})

/** Saves the permission differences of this compare's saved report as a CSV. */
async function exportPermissions(): Promise<void> {
  const reportId = result.value?.reportId
  if (!reportId || exportingPermissions.value) {
    return
  }
  const mine = resultSeq
  exportingPermissions.value = true
  try {
    const path = await exportReport(
      {
        kind: 'compare',
        id: reportId,
        title: 'Compare',
        createdAtMs: (finishedAt.value ?? new Date()).getTime(),
        job: currentJob(),
      },
      'csv',
      'permissions',
    )
    if (path && mine === resultSeq) {
      showToast(`Saved ${path}`)
    }
  } catch (err) {
    if (mine === resultSeq) {
      showToast(errorMessage(err))
    }
  } finally {
    if (mine === resultSeq) {
      exportingPermissions.value = false
    }
  }
}

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
  confirmingCopy.value = false
  selected.value = null
  sort.value = defaultSort
  // Anything still in flight for the previous result must not write to this one.
  resultSeq++
  exportingPermissions.value = false
  detail.value = null
  detailLoading.value = false
  await runCompareJob(left, right, ignoreJunk.value, comparePermissions.value)
  recent.value = loadRecent()
  if (result.value) {
    const mismatches = (result.value.permissions?.counts ?? []).some(
      (count) => count.status !== 'sameNameDifferentAccount',
    )
    filter.value = result.value.summary.missing > 0 ? 'missing' : mismatches ? 'permissions' : 'all'
    await nextTick()
    resultsEl.value?.scrollIntoView({ block: 'start', behavior: 'smooth' })
  }
}

async function cancel(): Promise<void> {
  try {
    await cancelCompare()
  } catch (err) {
    showToast(errorMessage(err))
  }
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
  try {
    await toggleRow(id)
  } catch (err) {
    showToast(errorMessage(err))
  }
}

const { toast, showToast } = useToast()

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
          to="/compare/readiness"
          class="inline-flex h-8 items-center rounded-md border border-stroke bg-card px-4 hover:bg-card-hover"
          title="Before you move a folder: list the names the target cannot hold and the paths that will be too long"
        >
          Readiness
        </RouterLink>
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
    <label
      class="mt-1 flex w-fit items-center gap-2 text-[13px] text-muted"
      title="Reads the owner, inheritance and access entries of every folder on both sides (and of files that have entries of their own). Read only; slower on big trees."
    >
      <input
        v-model="comparePermissions"
        type="checkbox"
        class="size-4 accent-accent"
        :disabled="running"
        data-testid="compare-permissions"
      />
      Compare permissions (owner, inheritance and entries; read only)
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
    <ShareSignIn
      :paths="[paths.left, paths.right]"
      :open="needsSignIn(error)"
    />
    <p
      v-if="notice"
      class="mt-4 flex items-center gap-2 text-[13px] text-muted"
      role="status"
    >
      <Info class="size-4 shrink-0" />
      {{ notice }}
    </p>

    <CompareProgress
      v-if="running"
      :progress="progress"
      :elapsed="elapsed"
      :permissions="permissionsProgress"
    />

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
          v-if="result.permissions?.status === 'notSupported'"
          class="flex items-center gap-2 text-muted"
        >
          <Info class="size-4 shrink-0" />
          Permissions can only be compared on Windows.
        </p>
        <p
          v-if="permissionsChecked && result.permissions?.serverNote"
          class="flex items-center gap-2 text-muted"
        >
          <Info class="size-4 shrink-0" />
          {{ result.permissions.serverNote }}
        </p>
        <p
          v-if="permissionsChecked && result.permissions?.itemsTruncated"
          class="flex items-center gap-2 text-warning"
        >
          <TriangleAlert class="size-4 shrink-0" />
          Many items differ: the table shows every status, but details and the saved report keep
          only the first ones.
        </p>
        <p
          v-if="cloudFiles > 0"
          class="flex items-center gap-2 text-muted"
        >
          <Info class="size-4 shrink-0" />
          {{ plural(cloudFiles, 'online-only cloud file') }}: size compared, nothing downloaded.
        </p>
      </div>

      <CompareToolbar
        :result="result"
        :compared="compared"
        :finished-at="finishedAt"
        :filter="filter"
        :confirming="confirmingCopy"
        :exporting="exportingPermissions"
        @filter="filter = $event"
        @report="openCompareReport"
        @export-permissions="exportPermissions"
        @copy-list="copyMissing"
        @copy-files="copyMissingFiles"
        @confirming="confirmingCopy = $event"
        @recheck="runCompare(compared)"
      />

      <CompareTable
        v-if="rows.length > 0"
        class="mt-3 min-h-72 flex-1"
        :rows="rows"
        :loading="loading"
        :sort="sort"
        :selected="selected"
        :show-permissions="permissionsChecked"
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
                  : filter === 'permissions'
                    ? 'No permission differences'
                    : 'Both folders are empty'
            }}
          </p>
          <p class="mt-1 text-[13px] text-muted">
            {{
              filter === 'missing'
                ? 'Every source file is at the destination.'
                : filter === 'changes'
                  ? 'Both folders hold the same files with the same sizes.'
                  : filter === 'permissions'
                    ? 'Owners, inheritance and entries match everywhere that was compared.'
                    : 'There is nothing to compare.'
            }}
          </p>
        </div>
      </div>
      <PermissionDetailPanel
        v-if="selectedRow && selectedCell && permissionsChecked"
        :name="selectedRow.row.name || 'the compared folder'"
        :cell="selectedCell"
        :detail="detail"
        :loading="detailLoading"
      />
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
</style>
