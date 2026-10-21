<script setup lang="ts">
import {
  ArrowUp,
  ChevronRight,
  Copy,
  File,
  FileText,
  Folder,
  FolderOpen,
  Info,
  Link2,
  LoaderCircle,
  RotateCw,
  ShieldAlert,
  Trash2,
  TriangleAlert,
} from '@lucide/vue'
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import BulkDeleteDialog from '@/components/BulkDeleteDialog.vue'
import ContextMenu, { type MenuItem } from '@/components/ContextMenu.vue'
import DriveTiles from '@/components/DriveTiles.vue'
import PageHeader from '@/components/PageHeader.vue'
import JobFields from '@/components/JobFields.vue'
import JunkPanel from '@/components/JunkPanel.vue'
import PathField from '@/components/PathField.vue'
import QuickWins from '@/components/QuickWins.vue'
import TreemapView from '@/components/TreemapView.vue'
import {
  appAdmin,
  canDelete,
  cancelScan,
  cleanupJob,
  driveFor,
  drives,
  itemPath,
  openFolder,
  openTrail,
  refreshDrives,
  removeItem,
  restartAsAdmin,
  revealItem,
  rowsFor,
  runScan,
  scanPercent,
  setSizeMode,
  shownBytes,
  sizeMode,
  type CleanupRow,
} from '@/lib/cleanup'
import { copyText } from '@/lib/clipboard'
import { cleanPath, errorMessage, pickFolder } from '@/lib/compare'
import { openReport } from '@/lib/reports'
import { categoryLabels, categoryOfExtension, type Category } from '@/lib/fileTypes'
import { formatBytes, formatDuration, plural } from '@/lib/format'

const job = cleanupJob
const folderInput = ref('')

/** The item the actions apply to, from the list, the treemap or "Largest files". */
interface Picked {
  id: number
  name: string
  size: number
  isFolder: boolean
  files: number
}
const picked = ref<Picked | null>(null)
watch(
  () => job.overview,
  () => {
    picked.value = null
  },
)

/** Rows as shown: `size` follows the Size on disk / Size toggle. */
const viewRows = computed(() => rowsFor(job.rows, sizeMode.value))
const hardLinkTitle =
  'Another name for this file is in the scan. Only one name carries the size on disk.'

function pick(row: CleanupRow): void {
  // Deleting frees disk space, so the picked item always carries the size on disk.
  const raw = job.rows.find((candidate) => candidate.id === row.id) ?? row
  picked.value = {
    id: raw.id,
    name: raw.name,
    size: raw.size,
    isFolder: raw.kind === 'dir',
    files: raw.files,
  }
}

const shownSize = computed(() => viewRows.value.reduce((sum, row) => sum + row.size, 0))
const scannedDrive = computed(() => driveFor(job.path, drives.value))
const percent = computed(() => scanPercent(job.progress?.bytes ?? 0, scannedDrive.value))

// Elapsed time while a scan runs.
const now = ref(Date.now())
const ticker = setInterval(() => {
  now.value = Date.now()
}, 250)
const elapsed = computed(() => {
  const seconds = Math.max(0, Math.floor((now.value - job.startedAt) / 1000))
  return `${String(Math.floor(seconds / 60))}:${String(seconds % 60).padStart(2, '0')}`
})

function scan(path: string): void {
  const clean = cleanPath(path)
  if (clean && !job.running) {
    picked.value = null
    void runScan(clean).then(refreshDrives)
  }
}

async function browse(): Promise<void> {
  const folder = await pickFolder('Choose a folder to scan', cleanPath(folderInput.value))
  if (folder) {
    folderInput.value = folder
    scan(folder)
  }
}

function newScan(): void {
  job.overview = null
  job.error = ''
  job.notice = ''
  void refreshDrives()
}

async function open(row: CleanupRow): Promise<void> {
  try {
    if (row.kind === 'dir' && row.hasChildren) {
      await openFolder(row)
      picked.value = null
    } else {
      await revealItem(row.id)
    }
  } catch (err) {
    showToast(errorMessage(err))
  }
}

async function goUp(index: number): Promise<void> {
  try {
    await openTrail(index)
    picked.value = null
  } catch (err) {
    showToast(errorMessage(err))
  }
}

async function openScanReport(): Promise<void> {
  if (!job.reportId) {
    return
  }
  try {
    await openReport({ kind: 'diskUsage', id: job.reportId })
  } catch (err) {
    showToast(errorMessage(err))
  }
}

async function reveal(id: number): Promise<void> {
  try {
    await revealItem(id)
  } catch (err) {
    showToast(errorMessage(err))
  }
}

async function copyPath(id: number): Promise<void> {
  try {
    await copyText(await itemPath(id))
    showToast('Path copied')
  } catch (err) {
    showToast(errorMessage(err))
  }
}

// Recycle / delete, after a confirm dialog with the size.
const confirmEl = ref<HTMLDialogElement | null>(null)
const pending = ref<{ item: Picked; permanent: boolean } | null>(null)
const deleting = ref(false)

function askDelete(item: Picked | null, permanent: boolean): void {
  if (!item || deleting.value) {
    return
  }
  if (!canDelete(1, permanent, appAdmin.elevated)) {
    showToast('Deleting permanently needs administrator. Use Restart as administrator first.')
    return
  }
  pending.value = { item, permanent }
  confirmEl.value?.showModal()
}

async function confirmDelete(): Promise<void> {
  const request = pending.value
  if (!request) {
    return
  }
  deleting.value = true
  try {
    await removeItem(request.item.id, request.permanent)
    showToast(
      `${request.permanent ? 'Deleted' : 'Moved to the Recycle Bin:'} ${request.item.name} (${formatBytes(request.item.size)})`,
    )
    picked.value = null
    void refreshDrives()
  } catch (err) {
    showToast(errorMessage(err))
  } finally {
    deleting.value = false
    confirmEl.value?.close()
    pending.value = null
  }
}

// Several items at once: tick rows, then review the dry run in the bulk dialog.
const checked = ref(new Set<number>())
const bulkEl = ref<InstanceType<typeof BulkDeleteDialog> | null>(null)
const junkEl = ref<InstanceType<typeof JunkPanel> | null>(null)
watch(
  () => [job.overview, job.trail.length],
  () => {
    checked.value = new Set()
  },
)

function toggleChecked(id: number): void {
  const next = new Set(checked.value)
  if (!next.delete(id)) {
    next.add(id)
  }
  checked.value = next
}

function reviewIds(ids: number[]): void {
  void bulkEl.value?.show(ids)
}

function onBulkFinished(): void {
  checked.value = new Set()
  picked.value = null
  void refreshDrives()
  void junkEl.value?.reload()
}

async function restartAdmin(): Promise<void> {
  try {
    await restartAsAdmin()
  } catch (err) {
    showToast(errorMessage(err))
  }
}

const menu = ref<{ item: Picked; x: number; y: number } | null>(null)
function openMenu(row: CleanupRow, x: number, y: number): void {
  pick(row)
  if (picked.value) {
    menu.value = { item: picked.value, x, y }
  }
}
const menuItems = computed<(MenuItem | null)[]>(() => {
  const item = menu.value?.item
  if (!item) {
    return []
  }
  return [
    { label: 'Show in Explorer', icon: FolderOpen, action: () => void reveal(item.id) },
    { label: 'Copy path', icon: Copy, action: () => void copyPath(item.id) },
    null,
    { label: 'Move to Recycle Bin', icon: Trash2, action: () => askDelete(item, false) },
    {
      label: appAdmin.elevated ? 'Delete permanently…' : 'Delete permanently (needs administrator)',
      disabled: !appAdmin.elevated,
      action: () => askDelete(item, true),
    },
  ]
})

// Lists under the treemap.
const tab = ref<'files' | 'types'>('files')
interface TypeGroup {
  category: Category
  size: number
  files: number
}
const typeGroups = computed<TypeGroup[]>(() => {
  const groups = new Map<Category, TypeGroup>()
  for (const type of job.overview?.fileTypes ?? []) {
    const category = categoryOfExtension(type.extension)
    const group = groups.get(category) ?? { category, size: 0, files: 0 }
    group.size += shownBytes(type, sizeMode.value)
    group.files += type.files
    groups.set(category, group)
  }
  return [...groups.values()].sort((a, b) => b.size - a.size)
})
const rootSize = computed(() => (job.overview ? shownBytes(job.overview.root, sizeMode.value) : 0))

function share(size: number, total: number): string {
  return total > 0 ? `${((size / total) * 100).toFixed(size / total >= 0.1 ? 0 : 1)}%` : '0%'
}

function modified(ms: number | null): string {
  return ms === null ? '' : new Date(ms).toLocaleDateString('en-US', { dateStyle: 'medium' })
}

// Short confirmation or error at the bottom of the window.
const toast = ref('')
let toastTimer: ReturnType<typeof setTimeout> | undefined
function showToast(message: string): void {
  toast.value = message
  clearTimeout(toastTimer)
  toastTimer = setTimeout(() => {
    toast.value = ''
  }, 4000)
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape' && job.running) {
    void cancelScan()
    return
  }
  const typing = event.target instanceof HTMLInputElement
  if (typing || menu.value || confirmEl.value?.open || !job.overview) {
    return
  }
  if (document.querySelector('dialog[open]')) {
    return
  }
  if (event.key === 'Delete') {
    askDelete(picked.value, event.shiftKey)
  } else if (event.key === 'Backspace' && job.trail.length > 1) {
    void goUp(job.trail.length - 2)
  }
}

const route = useRoute()
const router = useRouter()
onMounted(() => {
  window.addEventListener('keydown', onKeydown)
  void refreshDrives()
  // Home's drive tiles open this page with ?scan=C:\.
  const requested = route.query.scan
  if (typeof requested === 'string') {
    void router.replace({ query: {} })
    scan(requested)
  }
})
onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown)
  clearInterval(ticker)
  clearTimeout(toastTimer)
})
</script>

<template>
  <div class="page mx-auto flex min-h-full max-w-6xl flex-col px-6 py-6 lg:px-10 lg:py-8">
    <div class="flex items-start justify-between gap-6">
      <PageHeader
        title="Disk Cleanup"
        summary="See what fills a drive or folder, then move what you don't need to the Recycle Bin."
      />
      <button
        v-if="job.running"
        type="button"
        class="mt-1.5 h-8 shrink-0 rounded-md border border-stroke bg-card px-5 hover:bg-card-hover"
        title="Cancel (Esc)"
        @click="cancelScan"
      >
        Cancel
      </button>
      <div
        v-else-if="job.overview"
        class="mt-1.5 flex shrink-0 gap-2"
      >
        <button
          v-if="job.reportId"
          type="button"
          class="flex h-8 items-center gap-1.5 rounded-md border border-stroke bg-card px-4 hover:bg-card-hover"
          title="Open the saved report in the browser, with what you removed"
          @click="openScanReport"
        >
          <FileText
            class="size-4"
            :stroke-width="1.75"
          />
          Report
        </button>
        <button
          type="button"
          class="h-8 rounded-md border border-stroke bg-card px-4 hover:bg-card-hover"
          @click="newScan"
        >
          New scan
        </button>
        <button
          type="button"
          class="flex h-8 items-center gap-1.5 rounded-md bg-accent px-4 font-semibold text-on-accent hover:bg-accent-hover"
          :title="`Scan ${job.path} again`"
          @click="scan(job.path)"
        >
          <RotateCw class="size-4" />
          Rescan
        </button>
      </div>
    </div>

    <p
      v-if="job.error"
      class="mb-4 flex items-start gap-2 rounded-lg border border-danger/40 bg-danger-bg px-4 py-3 text-[13px] text-danger"
      role="alert"
    >
      <TriangleAlert class="mt-0.5 size-4 shrink-0" />
      {{ job.error }}
    </p>
    <p
      v-if="job.notice"
      class="mb-4 flex items-center gap-2 text-[13px] text-muted"
      role="status"
    >
      <Info class="size-4 shrink-0" />
      {{ job.notice }}
    </p>

    <!-- Scanning -->
    <section
      v-if="job.running"
      class="rounded-lg border border-stroke bg-card px-5 py-4"
      aria-live="polite"
    >
      <div class="flex items-center justify-between gap-4">
        <p class="flex min-w-0 items-center gap-2 font-semibold">
          <LoaderCircle class="size-4 shrink-0 animate-spin text-accent" />
          <span class="truncate">Scanning {{ job.path }}…</span>
        </p>
        <p class="shrink-0 text-xs text-muted tabular-nums">
          <template v-if="percent !== null">{{ percent }}% · </template>{{ elapsed }} · Esc to
          cancel
        </p>
      </div>
      <div
        v-if="percent !== null"
        class="mt-3 h-1 overflow-hidden rounded-full bg-subtle-strong"
      >
        <div
          class="h-full rounded-full bg-accent transition-[width]"
          :style="{ width: `${String(percent)}%` }"
        />
      </div>
      <div
        v-else
        class="indeterminate mt-3 h-1 overflow-hidden rounded-full bg-subtle-strong"
      />
      <p class="mt-3 tabular-nums">
        {{ plural(job.progress?.files ?? 0, 'file') }} · {{ formatBytes(job.progress?.bytes ?? 0) }}
      </p>
      <p
        class="truncate text-xs text-faint"
        :title="job.progress?.current"
      >
        {{ job.progress?.current || ' ' }}
      </p>
    </section>

    <!-- Result -->
    <div
      v-else-if="job.overview"
      class="flex flex-1 flex-col"
    >
      <div class="summary grid gap-3">
        <div class="tile">
          <p class="text-xs text-muted">Scanned</p>
          <p class="value">{{ formatBytes(rootSize) }}</p>
          <p
            class="detail"
            :title="job.path"
          >
            {{ job.path }}
          </p>
        </div>
        <div class="tile">
          <p class="text-xs text-muted">Files</p>
          <p class="value">{{ plural(job.overview.root.files, 'file') }}</p>
          <p class="detail">{{ plural(job.overview.root.dirs, 'folder') }}</p>
        </div>
        <div class="tile">
          <p class="text-xs text-muted">Free space</p>
          <p class="value">{{ scannedDrive ? formatBytes(scannedDrive.free) : '—' }}</p>
          <p class="detail">
            {{ scannedDrive ? `of ${formatBytes(scannedDrive.total)}` : 'Scan a drive to see it' }}
          </p>
        </div>
        <div class="tile">
          <p class="text-xs text-muted">Scan time</p>
          <p class="value">{{ formatDuration(job.elapsedMs) }}</p>
          <p class="detail">
            {{
              job.finishedAt?.toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' })
            }}
          </p>
        </div>
      </div>
      <p
        v-if="job.overview.root.errors > 0"
        class="mt-3 flex items-center gap-2 text-[13px] text-warning"
      >
        <TriangleAlert class="size-4 shrink-0" />
        {{ plural(job.overview.root.errors, 'folder') }} could not be read, so the totals may be
        low. Windows keeps some folders private even from administrators.
      </p>
      <p
        v-if="job.overview.root.cloudFiles > 0"
        class="mt-1 flex items-center gap-2 text-[13px] text-muted"
      >
        <Info class="size-4 shrink-0" />
        {{ plural(job.overview.root.cloudFiles, 'online-only cloud file') }} ({{
          formatBytes(job.overview.root.cloudBytes)
        }}) not counted: they take no space on this PC.
      </p>

      <!-- Where we are, and actions for the picked item -->
      <div class="mt-4 flex items-center gap-2">
        <button
          type="button"
          class="grid size-8 shrink-0 place-items-center rounded-md text-muted hover:bg-subtle-strong disabled:opacity-40"
          title="Up one folder (Backspace)"
          aria-label="Up one folder"
          :disabled="job.trail.length <= 1"
          @click="goUp(job.trail.length - 2)"
        >
          <ArrowUp class="size-4" />
        </button>
        <nav
          class="flex min-w-0 flex-1 items-center gap-0.5 overflow-hidden text-[13px]"
          aria-label="Folder"
        >
          <template
            v-for="(folder, index) in job.trail"
            :key="folder.id"
          >
            <ChevronRight
              v-if="index > 0"
              class="size-3.5 shrink-0 text-faint"
            />
            <button
              type="button"
              class="min-w-0 truncate rounded px-1.5 py-1 hover:bg-subtle-strong"
              :class="index === job.trail.length - 1 ? 'shrink-0 font-semibold' : 'text-muted'"
              :aria-current="index === job.trail.length - 1 ? 'location' : undefined"
              @click="goUp(index)"
            >
              {{ folder.name }}
            </button>
          </template>
          <span class="ml-2 shrink-0 text-xs text-muted tabular-nums">
            {{ formatBytes(shownSize) }}
          </span>
        </nav>
        <template v-if="checked.size > 0">
          <button
            v-if="!canDelete(checked.size, false, appAdmin.elevated)"
            type="button"
            class="flex h-8 shrink-0 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover"
            title="Deleting several items needs administrator. Windows asks first."
            @click="restartAdmin"
          >
            <ShieldAlert
              class="size-4"
              :stroke-width="1.75"
            />
            Restart as administrator
          </button>
          <button
            type="button"
            class="flex h-8 shrink-0 items-center gap-1.5 rounded-md border border-danger/40 bg-card px-3 text-[13px] text-danger hover:bg-danger-bg disabled:opacity-50"
            :disabled="!canDelete(checked.size, false, appAdmin.elevated)"
            @click="reviewIds([...checked])"
          >
            <Trash2
              class="size-4"
              :stroke-width="1.75"
            />
            Delete selected ({{ checked.size }})
          </button>
        </template>
        <template v-if="picked">
          <button
            type="button"
            class="flex h-8 shrink-0 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover"
            @click="reveal(picked.id)"
          >
            <FolderOpen
              class="size-4"
              :stroke-width="1.75"
            />
            Show in Explorer
          </button>
          <button
            type="button"
            class="flex h-8 shrink-0 items-center gap-1.5 rounded-md border border-danger/40 bg-card px-3 text-[13px] text-danger hover:bg-danger-bg"
            title="Move to the Recycle Bin (Delete). Shift+Delete deletes permanently."
            @click="askDelete(picked, false)"
          >
            <Trash2
              class="size-4"
              :stroke-width="1.75"
            />
            Recycle
          </button>
        </template>
      </div>

      <div class="split mt-2 grid gap-3">
        <TreemapView
          class="h-80"
          :rows="viewRows"
          :selected="picked?.id ?? null"
          @select="pick"
          @open="open"
          @menu="openMenu"
        />
        <ul
          class="h-80 overflow-y-auto rounded-lg border border-stroke bg-card"
          aria-label="Contents, largest first"
        >
          <li
            v-for="row in viewRows"
            :key="row.id"
            class="flex items-center"
          >
            <input
              type="checkbox"
              class="ml-3 shrink-0"
              :checked="checked.has(row.id)"
              :aria-label="`Select ${row.name}`"
              @change="toggleChecked(row.id)"
            />
            <button
              type="button"
              class="flex min-w-0 flex-1 items-center gap-2 px-3 py-1.5 text-left text-[13px] hover:bg-subtle"
              :class="{ 'bg-accent/12 hover:bg-accent/16': picked?.id === row.id }"
              :title="row.error ? `${row.name}: ${row.error}` : row.name"
              @click="pick(row)"
              @dblclick="open(row)"
              @keydown.enter="open(row)"
              @contextmenu.prevent="openMenu(row, $event.clientX, $event.clientY)"
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
                :title="hardLinkTitle"
                aria-label="Hard link"
              />
              <TriangleAlert
                v-if="row.errors > 0"
                class="size-3.5 shrink-0 text-warning"
              />
              <span class="w-11 shrink-0 text-right text-xs text-muted tabular-nums">
                {{ share(row.size, shownSize) }}
              </span>
              <span class="w-20 shrink-0 text-right tabular-nums">{{ formatBytes(row.size) }}</span>
            </button>
          </li>
          <li
            v-if="job.rows.length === 0"
            class="px-3 py-6 text-center text-[13px] text-muted"
          >
            This folder is empty.
          </li>
        </ul>
      </div>
      <p class="mt-2 text-xs text-faint">
        Click to pick, double-click to open a folder or show a file in Explorer, right-click for
        more. Delete recycles the picked item; Backspace goes up.
      </p>

      <div class="mt-5 flex flex-wrap items-center justify-between gap-3">
        <div
          class="flex w-fit rounded-md border border-stroke bg-subtle p-0.5"
          role="tablist"
        >
          <button
            v-for="option in [
              { value: 'files', label: 'Largest files' },
              { value: 'types', label: 'File types' },
            ] as const"
            :key="option.value"
            type="button"
            role="tab"
            :aria-selected="tab === option.value"
            class="flex h-7 items-center rounded px-3 text-[13px]"
            :class="tab === option.value ? 'bg-accent text-on-accent' : 'hover:bg-subtle-strong'"
            @click="tab = option.value"
          >
            {{ option.label }}
          </button>
        </div>
        <div
          class="flex w-fit rounded-md border border-stroke bg-subtle p-0.5"
          role="radiogroup"
          aria-label="Size shown"
          title="Size on disk is what deleting frees (whole clusters, compression, hard links once). Size is what Explorer calls Size."
        >
          <button
            v-for="option in [
              { value: 'disk', label: 'Size on disk' },
              { value: 'logical', label: 'Size' },
            ] as const"
            :key="option.value"
            type="button"
            role="radio"
            :aria-checked="sizeMode === option.value"
            class="flex h-7 items-center rounded px-3 text-[13px]"
            :class="
              sizeMode === option.value ? 'bg-accent text-on-accent' : 'hover:bg-subtle-strong'
            "
            @click="setSizeMode(option.value)"
          >
            {{ option.label }}
          </button>
        </div>
      </div>

      <ul
        v-if="tab === 'files'"
        class="mt-2 overflow-hidden rounded-lg border border-stroke bg-card"
        aria-label="Largest files"
      >
        <li
          v-for="file in job.overview.largestFiles"
          :key="file.id"
          class="border-b border-stroke last:border-b-0"
        >
          <button
            type="button"
            class="flex w-full items-center gap-3 px-3 py-1.5 text-left text-[13px] hover:bg-subtle"
            :class="{ 'bg-accent/12 hover:bg-accent/16': picked?.id === file.id }"
            :title="`${file.folder}\\${file.name}`"
            @click="
              picked = { id: file.id, name: file.name, size: file.size, isFolder: false, files: 1 }
            "
            @dblclick="reveal(file.id)"
            @contextmenu.prevent="
              menu = {
                item: { id: file.id, name: file.name, size: file.size, isFolder: false, files: 1 },
                x: $event.clientX,
                y: $event.clientY,
              }
            "
          >
            <span class="w-2/5 min-w-0 truncate">{{ file.name }}</span>
            <span class="min-w-0 flex-1 truncate text-xs text-muted">{{ file.folder }}</span>
            <Link2
              v-if="file.hardLinked"
              class="size-3.5 shrink-0 text-muted"
              :title="hardLinkTitle"
              aria-label="Hard link"
            />
            <span class="w-28 shrink-0 text-right text-xs text-muted tabular-nums">
              {{ modified(file.modifiedMs) }}
            </span>
            <span class="w-20 shrink-0 text-right tabular-nums">
              {{ formatBytes(shownBytes(file, sizeMode)) }}
            </span>
          </button>
        </li>
      </ul>

      <div
        v-else
        class="mt-2 grid gap-3 lg:grid-cols-2"
      >
        <ul
          class="rounded-lg border border-stroke bg-card p-3"
          aria-label="Space by type"
        >
          <li
            v-for="group in typeGroups"
            :key="group.category"
            class="py-1.5 text-[13px]"
          >
            <div class="flex items-center gap-2">
              <span
                class="size-3 shrink-0 rounded-sm"
                :style="{ background: `var(--app-type-${group.category})` }"
              />
              <span class="min-w-0 flex-1 truncate">{{ categoryLabels[group.category] }}</span>
              <span class="text-xs text-muted tabular-nums">{{ plural(group.files, 'file') }}</span>
              <span class="w-20 text-right tabular-nums">{{ formatBytes(group.size) }}</span>
            </div>
            <div class="mt-1 ml-5 h-1 overflow-hidden rounded-full bg-subtle-strong">
              <div
                class="h-full rounded-full"
                :style="{
                  width: share(group.size, rootSize),
                  background: `var(--app-type-${group.category})`,
                }"
              />
            </div>
          </li>
        </ul>
        <ul
          class="max-h-96 overflow-y-auto rounded-lg border border-stroke bg-card"
          aria-label="Space by extension"
        >
          <li
            v-for="type in job.overview.fileTypes.slice(0, 100)"
            :key="type.extension"
            class="flex items-center gap-2 border-b border-stroke px-3 py-1.5 text-[13px] last:border-b-0"
          >
            <span
              class="size-2.5 shrink-0 rounded-sm"
              :style="{ background: `var(--app-type-${categoryOfExtension(type.extension)})` }"
            />
            <span class="min-w-0 flex-1 truncate">
              {{ type.extension ? `.${type.extension}` : 'No extension' }}
            </span>
            <span class="text-xs text-muted tabular-nums">{{ plural(type.files, 'file') }}</span>
            <span class="w-11 text-right text-xs text-muted tabular-nums">
              {{ share(shownBytes(type, sizeMode), rootSize) }}
            </span>
            <span class="w-20 text-right tabular-nums">
              {{ formatBytes(shownBytes(type, sizeMode)) }}
            </span>
          </li>
        </ul>
      </div>

      <JunkPanel
        ref="junkEl"
        class="mt-6"
        @review="reviewIds"
      />
    </div>

    <!-- Start: pick a drive or folder -->
    <div
      v-else
      class="flex flex-col gap-6"
    >
      <JobFields />
      <section v-if="drives.length > 0">
        <h2 class="mb-2 text-[13px] font-semibold text-muted">Drives on this PC</h2>
        <DriveTiles
          :drives="drives"
          @pick="scan($event.path)"
        />
      </section>
      <section>
        <h2 class="mb-2 text-[13px] font-semibold text-muted">Or scan one folder</h2>
        <div class="flex items-end gap-2">
          <PathField
            v-model="folderInput"
            class="flex-1"
            label="Folder"
            hint="A user's profile, a share, a backup folder…"
            @browse="browse"
            @submit="scan(folderInput)"
          />
          <button
            type="button"
            class="mb-3 h-8 shrink-0 rounded-md bg-accent px-5 font-semibold text-on-accent hover:bg-accent-hover disabled:opacity-50"
            :disabled="!cleanPath(folderInput)"
            @click="scan(folderInput)"
          >
            Scan
          </button>
        </div>
        <p class="mt-2 text-xs text-faint">
          Nothing is deleted without asking. Deleted items go to the Recycle Bin unless you choose
          Delete permanently.
        </p>
      </section>
      <QuickWins @cleaned="refreshDrives" />
    </div>

    <dialog
      ref="confirmEl"
      class="m-auto w-[min(28rem,90vw)] rounded-lg border border-stroke-strong bg-app p-5 text-fg shadow-xl backdrop:bg-fg/20"
      @close="pending = null"
    >
      <template v-if="pending">
        <h2 class="font-display text-lg font-semibold">
          {{ pending.permanent ? 'Delete permanently?' : 'Move to the Recycle Bin?' }}
        </h2>
        <p class="mt-2 text-[13px] break-all">
          <span class="font-semibold">{{ pending.item.name }}</span>
        </p>
        <p class="mt-1 text-[13px] text-muted">
          {{ formatBytes(pending.item.size) }}
          <template v-if="pending.item.isFolder">
            · {{ plural(pending.item.files, 'file') }}</template
          >
        </p>
        <p
          v-if="pending.permanent"
          class="mt-3 text-[13px] text-danger"
        >
          This skips the Recycle Bin and cannot be undone.
        </p>
        <div class="mt-5 flex justify-end gap-2">
          <button
            type="button"
            class="h-8 rounded-md border border-stroke bg-card px-4 hover:bg-card-hover"
            :disabled="deleting"
            @click="confirmEl?.close()"
          >
            Cancel
          </button>
          <button
            type="button"
            class="flex h-8 items-center gap-1.5 rounded-md px-4 font-semibold disabled:opacity-60"
            :class="
              pending.permanent
                ? 'bg-danger text-on-accent'
                : 'bg-accent text-on-accent hover:bg-accent-hover'
            "
            :disabled="deleting"
            @click="confirmDelete"
          >
            <LoaderCircle
              v-if="deleting"
              class="size-4 animate-spin"
            />
            {{ pending.permanent ? 'Delete' : 'Recycle' }}
          </button>
        </div>
      </template>
    </dialog>

    <BulkDeleteDialog
      ref="bulkEl"
      @finished="onBulkFinished"
    />

    <ContextMenu
      v-if="menu"
      :items="menuItems"
      :x="menu.x"
      :y="menu.y"
      @close="menu = null"
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

.summary {
  grid-template-columns: repeat(2, minmax(0, 1fr));
}

.split {
  grid-template-columns: minmax(0, 1fr);
}

@container page (min-width: 46rem) {
  .summary {
    grid-template-columns: repeat(4, minmax(0, 1fr));
  }

  .split {
    grid-template-columns: minmax(0, 3fr) minmax(0, 2fr);
  }
}

.tile {
  min-width: 0;
  border: 1px solid var(--app-stroke);
  border-radius: 0.5rem;
  background: var(--app-card);
  padding: 0.625rem 1rem;
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
</style>
