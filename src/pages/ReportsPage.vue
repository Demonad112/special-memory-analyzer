<script setup lang="ts">
import {
  Brush,
  Copy,
  ExternalLink,
  FileSpreadsheet,
  FileText,
  FolderSync,
  HardDrive,
  LoaderCircle,
  Search,
  Trash2,
} from '@lucide/vue'
import { isTauri } from '@tauri-apps/api/core'
import { computed, onBeforeUnmount, onMounted, ref, type Component } from 'vue'
import { useRouter } from 'vue-router'
import PageHeader from '@/components/PageHeader.vue'
import { errorMessage } from '@/lib/compare'
import { jobLabel } from '@/lib/job'
import {
  deleteReport,
  exportReport,
  kindLabels,
  listReports,
  pruneReports,
  openReport,
  type ExportFormat,
  type ReportEntry,
  type ReportKind,
} from '@/lib/reports'

const router = useRouter()
const entries = ref<ReportEntry[]>([])
const loading = ref(true)
const error = ref('')
const filter = ref<ReportKind | 'all'>('all')
const query = ref('')

const icons: Record<ReportKind, Component> = {
  compare: Copy,
  record: FolderSync,
  diskUsage: HardDrive,
  cleanup: Brush,
}

const filters: { value: ReportKind | 'all'; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'compare', label: 'Compare' },
  { value: 'record', label: 'Record' },
  { value: 'diskUsage', label: 'Disk usage' },
  { value: 'cleanup', label: 'Cleanup' },
]

const shown = computed(() => {
  const words = query.value.toLowerCase().split(/\s+/).filter(Boolean)
  return entries.value.filter((entry) => {
    if (filter.value !== 'all' && entry.kind !== filter.value) {
      return false
    }
    const text = [
      entry.title,
      entry.subject,
      entry.headline,
      entry.job.client,
      entry.job.ticket,
      entry.job.technician,
    ]
      .join(' ')
      .toLowerCase()
    return words.every((word) => text.includes(word))
  })
})

async function refresh(): Promise<void> {
  if (!isTauri()) {
    loading.value = false
    return
  }
  try {
    entries.value = await listReports()
    error.value = ''
  } catch (err) {
    error.value = errorMessage(err)
  } finally {
    loading.value = false
  }
}

function when(ms: number): string {
  return new Date(ms).toLocaleString('en-US', { dateStyle: 'medium', timeStyle: 'short' })
}

async function open(entry: ReportEntry): Promise<void> {
  if (entry.kind === 'record') {
    void router.push(`/compare/record/${entry.id}`)
    return
  }
  try {
    await openReport(entry)
  } catch (err) {
    showToast(errorMessage(err))
  }
}

async function save(entry: ReportEntry, format: ExportFormat): Promise<void> {
  try {
    const path = await exportReport(entry, format)
    if (path) {
      showToast(`Saved ${path}`)
    }
  } catch (err) {
    showToast(errorMessage(err))
  }
}

const confirmEl = ref<HTMLDialogElement | null>(null)
const pending = ref<ReportEntry | null>(null)
function askDelete(entry: ReportEntry): void {
  pending.value = entry
  confirmEl.value?.showModal()
}
async function confirmDelete(): Promise<void> {
  const entry = pending.value
  confirmEl.value?.close()
  if (!entry) {
    return
  }
  try {
    await deleteReport(entry)
    entries.value = entries.value.filter(
      (item) => !(item.id === entry.id && item.kind === entry.kind),
    )
    showToast('Report deleted')
  } catch (err) {
    showToast(errorMessage(err))
  }
}

const toast = ref('')
let toastTimer: ReturnType<typeof setTimeout> | undefined
function showToast(message: string): void {
  toast.value = message
  clearTimeout(toastTimer)
  toastTimer = setTimeout(() => {
    toast.value = ''
  }, 4000)
}

onMounted(async () => {
  await pruneReports()
  await refresh()
})
onBeforeUnmount(() => clearTimeout(toastTimer))
</script>

<template>
  <div class="mx-auto flex min-h-full max-w-6xl flex-col px-6 py-6 lg:px-10 lg:py-8">
    <PageHeader
      title="Reports"
      summary="Every compare, record and cleanup, saved on this PC. Open one, or export it for the client."
    />

    <div class="flex flex-wrap items-center gap-3">
      <div
        class="flex rounded-md border border-stroke bg-subtle p-0.5"
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
          @click="filter = option.value"
        >
          {{ option.label }}
        </button>
      </div>
      <label class="relative min-w-48 flex-1">
        <span class="sr-only">Search reports</span>
        <Search class="pointer-events-none absolute top-2 left-2.5 size-4 text-faint" />
        <input
          v-model="query"
          type="search"
          spellcheck="false"
          placeholder="Search by client, ticket or folder"
          class="h-8 w-full rounded-md border border-stroke border-b-stroke-strong bg-card pr-2.5 pl-8 text-[13px] outline-none placeholder:text-faint focus:border-b-2 focus:border-b-accent"
        />
      </label>
    </div>

    <p
      v-if="error"
      class="mt-4 rounded-lg border border-danger/40 bg-danger-bg px-4 py-3 text-[13px] text-danger"
      role="alert"
    >
      {{ error }}
    </p>

    <div
      v-if="loading"
      class="mt-10 flex justify-center text-muted"
    >
      <LoaderCircle class="size-5 animate-spin" />
    </div>
    <ul
      v-else-if="shown.length > 0"
      class="mt-4 overflow-hidden rounded-lg border border-stroke bg-card"
      aria-label="Reports, newest first"
    >
      <li
        v-for="entry in shown"
        :key="`${entry.kind}-${entry.id}`"
        class="group flex items-center gap-3 border-b border-stroke px-4 py-2.5 last:border-b-0 hover:bg-subtle"
      >
        <component
          :is="icons[entry.kind]"
          class="size-5 shrink-0 text-muted"
          :stroke-width="1.5"
        />
        <button
          type="button"
          class="min-w-0 flex-1 text-left"
          :title="entry.kind === 'record' ? 'Open the record' : 'Open in the browser'"
          @click="open(entry)"
        >
          <span class="flex items-baseline gap-2">
            <span class="font-semibold">{{ entry.title }}</span>
            <span
              v-if="jobLabel(entry.job)"
              class="truncate text-xs text-accent"
            >
              {{ jobLabel(entry.job) }}
            </span>
          </span>
          <span
            class="block truncate text-xs text-muted"
            :title="entry.subject"
          >
            {{ entry.subject }}
          </span>
        </button>
        <span class="w-56 shrink-0 text-right text-[13px]">
          <span
            class="block truncate"
            :class="entry.problem ? 'text-danger' : ''"
          >
            {{ entry.headline }}
          </span>
          <span class="block text-xs text-faint tabular-nums">{{ when(entry.createdAtMs) }}</span>
        </span>
        <span
          class="flex shrink-0 gap-0.5 opacity-60 group-hover:opacity-100 focus-within:opacity-100"
        >
          <button
            v-if="entry.kind !== 'record'"
            type="button"
            class="grid size-8 place-items-center rounded-md hover:bg-subtle-strong"
            title="Open in the browser (print or save as PDF from there)"
            :aria-label="`Open ${kindLabels[entry.kind]} report in the browser`"
            @click="open(entry)"
          >
            <ExternalLink class="size-4" />
          </button>
          <button
            type="button"
            class="grid size-8 place-items-center rounded-md hover:bg-subtle-strong"
            title="Save as a web page for the client"
            aria-label="Export as HTML"
            @click="save(entry, 'html')"
          >
            <FileText class="size-4" />
          </button>
          <button
            type="button"
            class="grid size-8 place-items-center rounded-md hover:bg-subtle-strong"
            title="Save the list as CSV for Excel"
            aria-label="Export as CSV"
            @click="save(entry, 'csv')"
          >
            <FileSpreadsheet class="size-4" />
          </button>
          <button
            type="button"
            class="grid size-8 place-items-center rounded-md text-danger hover:bg-danger-bg"
            title="Delete"
            aria-label="Delete report"
            @click="askDelete(entry)"
          >
            <Trash2 class="size-4" />
          </button>
        </span>
      </li>
    </ul>
    <div
      v-else
      class="mt-4 grid flex-1 place-items-center rounded-lg border border-dashed border-stroke-strong px-6 py-12 text-center"
    >
      <div>
        <FileText
          class="mx-auto size-10 text-faint"
          :stroke-width="1.25"
        />
        <p class="mt-3 font-semibold">
          {{ entries.length > 0 ? 'No reports match' : 'No reports yet' }}
        </p>
        <p class="mt-1 max-w-md text-[13px] text-muted">
          {{
            entries.length > 0
              ? 'Try another filter or search.'
              : 'Each compare, record, disk scan and quick cleanup is saved here automatically.'
          }}
        </p>
      </div>
    </div>

    <dialog
      ref="confirmEl"
      class="m-auto w-[min(28rem,90vw)] rounded-lg border border-stroke-strong bg-app p-5 text-fg shadow-xl backdrop:bg-fg/20"
      @close="pending = null"
    >
      <template v-if="pending">
        <h2 class="font-display text-lg font-semibold">Delete this report?</h2>
        <p class="mt-2 text-[13px]">
          <span class="font-semibold">{{ pending.title }}</span> · {{ when(pending.createdAtMs) }}
        </p>
        <p class="mt-1 text-[13px] break-all text-muted">{{ pending.subject }}</p>
        <p class="mt-3 text-[13px] text-danger">
          {{
            pending.kind === 'record'
              ? 'This deletes the record and its file lists, which are your proof of the copy. It cannot be undone.'
              : 'It cannot be undone. Export it first if the client may need it.'
          }}
        </p>
        <div class="mt-5 flex justify-end gap-2">
          <button
            type="button"
            class="h-8 rounded-md border border-stroke bg-card px-4 hover:bg-card-hover"
            @click="confirmEl?.close()"
          >
            Cancel
          </button>
          <button
            type="button"
            class="h-8 rounded-md bg-danger px-4 font-semibold text-on-accent"
            @click="confirmDelete"
          >
            Delete
          </button>
        </div>
      </template>
    </dialog>

    <Transition name="toast">
      <p
        v-if="toast"
        class="fixed bottom-5 left-1/2 z-40 max-w-[min(32rem,90vw)] -translate-x-1/2 rounded-md bg-fg px-4 py-2 text-[13px] break-all text-app shadow-lg"
        role="status"
      >
        {{ toast }}
      </p>
    </Transition>
  </div>
</template>

<style scoped>
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
