<script setup lang="ts">
import {
  ArrowLeft,
  ChevronRight,
  CircleCheck,
  File,
  FileText,
  Folder,
  FolderOpen,
  Info,
  Link2,
  LoaderCircle,
  TriangleAlert,
} from '@lucide/vue'
import { computed, onBeforeUnmount, onMounted, reactive, watch } from 'vue'
import JobFields from '@/components/JobFields.vue'
import PageHeader from '@/components/PageHeader.vue'
import PathField from '@/components/PathField.vue'
import { cleanPath, pickFolder } from '@/lib/compare'
import { errorMessage } from '@/lib/errors'
import { formatBytes, plural } from '@/lib/format'
import {
  cancelReadiness,
  displayName,
  groupCounts,
  loadForm,
  readinessJob,
  revealItem,
  runReadinessJob,
  saveForm,
  setFilter,
  targetLabels,
  toggleRow,
  visibleRows,
  type ProblemKind,
  type ReadinessRow,
  type TargetProfile,
} from '@/lib/readiness'
import { openReport } from '@/lib/reports'
import { useElapsed } from '@/lib/useElapsed'
import { useToast } from '@/lib/useToast'

const form = reactive(loadForm())
watch(form, () => {
  saveForm({ ...form })
})

const { toast, showToast } = useToast()
const running = computed(() => readinessJob.running)
const result = computed(() => readinessJob.result)
const report = computed(() => result.value?.report ?? null)
const groups = computed(() => (report.value ? groupCounts(report.value) : []))
const canStart = computed(() => !running.value && cleanPath(form.path) !== '')
const elapsed = useElapsed(
  () => readinessJob.startedAt,
  () => readinessJob.running,
)
const rows = computed(() =>
  visibleRows(readinessJob.rows, readinessJob.children, readinessJob.expanded),
)
const rootOwn = computed(() => readinessJob.root?.own ?? [])
const targets = Object.entries(targetLabels) as [TargetProfile, string][]
const phaseText = computed(() => {
  switch (readinessJob.progress?.phase) {
    case 'names':
      return 'Checking names…'
    case 'inUse':
      return 'Looking for files in use…'
    default:
      return 'Reading the folder…'
  }
})
const inUseText = computed(() => {
  const pass = report.value?.inUse
  switch (pass?.status) {
    case 'checked':
      return `${plural(pass.checked, 'file')} opened to look for files in use.`
    case 'notSupported':
      return 'Files in use were not checked on this system.'
    default:
      return 'Files in use were not checked.'
  }
})

async function start(): Promise<void> {
  if (!canStart.value) {
    return
  }
  form.path = cleanPath(form.path)
  form.destination = cleanPath(form.destination)
  await runReadinessJob({ ...form })
}

async function browse(field: 'path' | 'destination'): Promise<void> {
  const picked = await pickFolder(
    field === 'path' ? 'Choose the folder to check' : 'Choose the planned destination folder',
    form[field],
  )
  if (picked) {
    form[field] = picked
  }
}

function cancel(): void {
  void cancelReadiness()
}

function onKey(event: KeyboardEvent): void {
  if (event.key === 'Escape' && running.value) {
    cancel()
  }
}
onMounted(() => {
  window.addEventListener('keydown', onKey)
})
onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKey)
})

async function filterBy(kind: ProblemKind | null): Promise<void> {
  try {
    await setFilter(kind)
  } catch (err) {
    showToast(errorMessage(err))
  }
}

async function open(row: ReadinessRow): Promise<void> {
  if (!row.hasChildren) {
    return
  }
  try {
    await toggleRow(row.id)
  } catch (err) {
    showToast(errorMessage(err))
  }
}

async function reveal(row: ReadinessRow): Promise<void> {
  try {
    await revealItem(row.id)
  } catch (err) {
    showToast(errorMessage(err))
  }
}

async function openSaved(): Promise<void> {
  const id = result.value?.reportId
  if (!id) {
    return
  }
  try {
    await openReport({ kind: 'readiness', id })
  } catch (err) {
    showToast(errorMessage(err))
  }
}

function rowIcon(row: ReadinessRow): typeof File {
  return row.kind === 'dir' ? Folder : row.kind === 'link' ? Link2 : File
}
</script>

<template>
  <div class="page mx-auto flex min-h-full max-w-6xl flex-col px-6 py-6 lg:px-10 lg:py-8">
    <RouterLink
      to="/compare"
      class="mb-2 flex w-fit items-center gap-1 text-[13px] text-muted hover:text-fg"
    >
      <ArrowLeft class="size-4" />
      Compare &amp; Record
    </RouterLink>
    <div class="flex items-start justify-between gap-6">
      <PageHeader
        title="Migration readiness"
        summary="Check a folder before you move it: names the target cannot hold, paths that will be too long there, and files in use."
      />
      <button
        v-if="!running"
        type="button"
        class="mt-1.5 h-8 shrink-0 rounded-md bg-accent px-5 font-semibold text-on-accent hover:bg-accent-hover disabled:opacity-50"
        :disabled="!canStart"
        @click="start"
      >
        Start
      </button>
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

    <div class="grid grid-cols-2 gap-3">
      <PathField
        v-model="form.path"
        label="Source"
        hint="The folder you are going to move"
        :disabled="running"
        @browse="browse('path')"
        @submit="start"
      />
      <PathField
        v-model="form.destination"
        label="Destination root"
        hint="Optional: paths are measured as they will be here"
        :disabled="running"
        @browse="browse('destination')"
        @submit="start"
      />
    </div>
    <fieldset
      class="mt-3 flex flex-wrap items-center gap-4 text-[13px]"
      :disabled="running"
    >
      <legend class="sr-only">Target</legend>
      <span class="font-semibold">Target</span>
      <label
        v-for="[value, label] in targets"
        :key="value"
        class="flex items-center gap-2"
      >
        <input
          v-model="form.target"
          type="radio"
          name="target"
          :value="value"
          class="size-4 accent-accent"
        />
        {{ label }}
      </label>
      <label class="ml-auto flex items-center gap-2 text-muted">
        <input
          v-model="form.checkInUse"
          type="checkbox"
          class="size-4 accent-accent"
        />
        Find files in use (opens each file for read; slower on big shares)
      </label>
    </fieldset>
    <JobFields
      class="mt-3"
      :disabled="running"
    />

    <p
      v-if="readinessJob.error"
      class="mt-4 flex items-start gap-2 rounded-lg border border-danger/40 bg-danger-bg px-4 py-3 text-[13px] text-danger"
      role="alert"
    >
      <TriangleAlert class="mt-0.5 size-4 shrink-0" />
      {{ readinessJob.error }}
    </p>
    <p
      v-if="readinessJob.notice"
      class="mt-4 flex items-center gap-2 text-[13px] text-muted"
      role="status"
    >
      <Info class="size-4 shrink-0" />
      {{ readinessJob.notice }}
    </p>

    <section
      v-if="running"
      class="mt-6 rounded-lg border border-stroke bg-card px-5 py-4"
      aria-live="polite"
    >
      <div class="flex items-center justify-between gap-4">
        <p class="flex items-center gap-2 font-semibold">
          <LoaderCircle class="size-4 animate-spin text-accent" />
          {{ phaseText }}
        </p>
        <p class="text-xs text-muted tabular-nums">{{ elapsed }} · Esc to cancel</p>
      </div>
      <p
        v-if="readinessJob.progress?.phase === 'inUse'"
        class="mt-3 tabular-nums"
      >
        {{ readinessJob.progress.checked.toLocaleString() }} of
        {{ plural(readinessJob.progress.toCheck, 'file') }} opened ·
        {{ readinessJob.progress.inUse.toLocaleString() }} in use
      </p>
      <p
        v-else
        class="mt-3 tabular-nums"
      >
        {{ plural(readinessJob.progress?.scan.files ?? 0, 'file') }} ·
        {{ formatBytes(readinessJob.progress?.scan.bytes ?? 0) }}
      </p>
      <p
        class="truncate text-xs text-faint"
        :title="readinessJob.progress?.scan.current"
      >
        {{
          readinessJob.progress?.phase === 'inUse'
            ? ' '
            : readinessJob.progress?.scan.current || ' '
        }}
      </p>
    </section>

    <div
      v-else-if="report"
      class="flex flex-1 flex-col pt-5"
    >
      <div class="flex flex-wrap items-center gap-2">
        <button
          type="button"
          class="h-8 rounded-md border px-3 text-[13px]"
          :class="
            readinessJob.filter === null
              ? 'border-accent bg-accent/12'
              : 'border-stroke bg-card hover:bg-card-hover'
          "
          @click="filterBy(null)"
        >
          All · {{ report.problemsTotal }}
        </button>
        <button
          v-for="group in groups"
          :key="group.kind"
          type="button"
          class="h-8 rounded-md border px-3 text-[13px]"
          :class="
            readinessJob.filter === group.kind
              ? 'border-accent bg-accent/12'
              : 'border-stroke bg-card hover:bg-card-hover'
          "
          @click="filterBy(group.kind)"
        >
          <span :class="group.blockers > 0 ? 'text-danger' : ''">{{ group.heading }}</span>
          · {{ group.count }}
        </button>
        <span class="flex-1" />
        <button
          v-if="result?.reportId"
          type="button"
          class="flex h-8 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover"
          @click="openSaved"
        >
          <FileText class="size-4" />
          Open the report
        </button>
      </div>
      <p class="mt-3 text-[13px] text-muted">
        {{ plural(report.files, 'file') }} and {{ plural(report.dirs, 'folder') }} checked for
        {{ targetLabels[report.target] }}.
        {{
          report.destinationRoot
            ? `Path lengths are measured under ${report.destinationRoot}.`
            : 'Path lengths are measured at the source.'
        }}
        <template v-if="report.cloudFiles > 0">
          {{ plural(report.cloudFiles, 'online-only file') }} checked by name only.
        </template>
        {{ inUseText }}
      </p>
      <p
        v-if="report.inUse.inUse > 0"
        class="mt-3 flex items-start gap-2 rounded-lg border border-stroke bg-card px-4 py-3 text-[13px]"
      >
        <Info class="mt-0.5 size-4 shrink-0 text-warning" />
        <span>
          {{ plural(report.inUse.inUse, 'file') }} {{ report.inUse.inUse === 1 ? 'was' : 'were' }}
          open in another program during this check. This is a moment in time: close the program
          before the copy, or copy with another tool and record it in Watch mode.
        </span>
      </p>

      <p
        v-if="report.problemsTotal === 0"
        class="mt-4 flex items-center gap-2 rounded-lg border border-stroke bg-card px-4 py-3 text-success"
      >
        <CircleCheck class="size-4" />
        No problems found. Every name and path fits the target.
      </p>
      <ul
        v-else
        class="mt-3 overflow-hidden rounded-lg border border-stroke bg-card"
      >
        <li
          v-for="problem in rootOwn"
          :key="`root-${problem.rule}`"
          class="border-b border-stroke px-4 py-2 text-[13px]"
        >
          <span class="font-semibold">The source folder</span>:
          <span :class="problem.blocker ? 'text-danger' : 'text-warning'">{{ problem.label }}</span>
          — {{ problem.hint }}
        </li>
        <li
          v-for="{ row, depth } in rows"
          :key="row.id"
          class="border-b border-stroke last:border-b-0"
        >
          <div
            class="group flex items-start gap-2 py-2 pr-4 text-[13px] hover:bg-card-hover"
            :style="{ paddingLeft: `${16 + depth * 20}px` }"
          >
            <button
              type="button"
              class="grid size-5 shrink-0 place-items-center rounded text-muted disabled:invisible"
              :disabled="!row.hasChildren"
              :aria-label="readinessJob.expanded.has(row.id) ? 'Close' : 'Open'"
              :aria-expanded="readinessJob.expanded.has(row.id)"
              @click="open(row)"
            >
              <LoaderCircle
                v-if="readinessJob.loading.has(row.id)"
                class="size-3.5 animate-spin"
              />
              <ChevronRight
                v-else
                class="size-3.5 transition-transform"
                :class="readinessJob.expanded.has(row.id) ? 'rotate-90' : ''"
              />
            </button>
            <component
              :is="rowIcon(row)"
              class="mt-0.5 size-4 shrink-0 text-muted"
              :stroke-width="1.75"
            />
            <div class="min-w-0 flex-1">
              <button
                type="button"
                class="max-w-full truncate text-left"
                :title="row.name"
                @click="open(row)"
              >
                {{ displayName(row.name) }}
              </button>
              <p
                v-for="problem in row.own"
                :key="problem.rule + problem.hint"
                class="text-xs"
              >
                <span :class="problem.blocker ? 'text-danger' : 'text-warning'">{{
                  problem.label
                }}</span>
                <span class="text-muted"> — {{ problem.hint }}</span>
              </p>
            </div>
            <span
              v-if="row.hasChildren"
              class="shrink-0 text-xs text-muted tabular-nums"
            >
              {{ plural(row.problems, 'problem') }}
            </span>
            <button
              type="button"
              class="grid size-6 shrink-0 place-items-center rounded text-muted opacity-0 group-hover:opacity-100 hover:bg-subtle-strong focus:opacity-100"
              title="Show in Explorer"
              aria-label="Show in Explorer"
              @click="reveal(row)"
            >
              <FolderOpen class="size-3.5" />
            </button>
          </div>
        </li>
      </ul>
    </div>
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
