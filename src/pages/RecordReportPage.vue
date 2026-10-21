<script setup lang="ts">
import {
  ArrowLeft,
  CircleCheck,
  ClipboardList,
  ExternalLink,
  FileSpreadsheet,
  FileText,
  FolderInput,
  FolderOpen,
  LoaderCircle,
  RotateCcw,
  TriangleAlert,
} from '@lucide/vue'
import { computed, onMounted, ref, shallowRef, watch } from 'vue'
import PageHeader from '@/components/PageHeader.vue'
import { copyText } from '@/lib/clipboard'
import { errorMessage, pickFolder } from '@/lib/compare'
import { formatBytes, formatCount, plural } from '@/lib/format'
import { exportReport, openReport, type ExportFormat } from '@/lib/reports'
import {
  cancelRecord,
  countByReason,
  formatNotCopiedList,
  isCancelled,
  loadRecord,
  reasonTitle,
  type FailureReason,
  recordJob,
  recoverRecord,
  retryRecord,
  revealPath,
  type RunDetails,
} from '@/lib/record'

const props = defineProps<{ id: string }>()

/** Rows drawn in the table; the rest are in the copied list and the recovery folder's CSV. */
const shownRows = 500

const details = shallowRef<RunDetails | null>(null)
const error = ref('')
const notice = ref('')
const recovering = ref(false)
const job = recordJob

async function load(): Promise<void> {
  error.value = ''
  try {
    details.value = await loadRecord(props.id)
  } catch (err) {
    error.value = errorMessage(err)
  }
}
onMounted(load)
watch(() => props.id, load)

const summary = computed(() => details.value?.summary)
const totals = computed(() => summary.value?.totals)
const reasons = computed(() => countByReason(details.value?.notCopied ?? []))
const finishedOk = computed(
  () => summary.value?.state === 'completed' && totals.value?.notCopied === 0,
)
const busy = computed(() => job.running || recovering.value)

async function openInBrowser(): Promise<void> {
  try {
    await openReport({ kind: 'record', id: props.id })
  } catch (err) {
    error.value = errorMessage(err)
  }
}

async function saveAs(format: ExportFormat): Promise<void> {
  const run = summary.value
  if (!run) {
    return
  }
  try {
    const path = await exportReport(
      {
        kind: 'record',
        id: props.id,
        title: run.settings.mode === 'watch' ? 'Watched copy' : 'Copy for me',
        createdAtMs: run.createdAtMs,
        job: details.value?.job ?? { client: '', ticket: '', technician: '' },
      },
      format,
    )
    if (path) {
      notice.value = `Saved ${path}`
    }
  } catch (err) {
    error.value = errorMessage(err)
  }
}

const stateText = computed(() => {
  switch (summary.value?.state) {
    case 'completed':
      return 'Finished'
    case 'cancelled':
      return 'Cancelled'
    case 'failed':
      return 'Stopped on an error'
    case 'prepared':
      return 'Not started'
    default:
      return 'Running'
  }
})

function when(ms: number | null | undefined): string {
  return ms
    ? new Date(ms).toLocaleString('en-US', { dateStyle: 'medium', timeStyle: 'short' })
    : '—'
}

async function retry(reason: FailureReason | null = null): Promise<void> {
  notice.value = ''
  error.value = ''
  job.running = true
  job.mode = 'copy'
  job.progress = null
  job.copyStartedAt = Date.now()
  job.finishing = false
  try {
    const before = totals.value?.notCopied ?? 0
    details.value = await retryRecord(props.id, [], (progress) => (job.progress = progress), reason)
    const fixed = before - details.value.summary.totals.notCopied
    notice.value =
      details.value.summary.totals.notCopied === 0
        ? `Retry copied ${plural(fixed, 'item')}. Nothing is missing now.`
        : `Retry copied ${plural(Math.max(fixed, 0), 'item')}; ${plural(details.value.summary.totals.notCopied, 'item')} still not copied.`
  } catch (err) {
    if (!isCancelled(err)) {
      error.value = errorMessage(err)
    }
    await load()
  } finally {
    job.running = false
  }
}

const recoveryFolder = ref('')
async function recover(): Promise<void> {
  const destination = summary.value?.settings.destination ?? ''
  const parent = destination.replace(/[\\/][^\\/]*[\\/]?$/, '')
  const folder = await pickFolder('Copy the missed files to', parent || undefined)
  if (!folder) {
    return
  }
  notice.value = ''
  error.value = ''
  recovering.value = true
  try {
    const result = await recoverRecord(props.id, folder)
    recoveryFolder.value = result.folder
    notice.value =
      result.skipped.length === 0
        ? `Copied ${plural(result.copied, 'file')} to ${result.folder}, with a list of them.`
        : `Copied ${plural(result.copied, 'file')} to ${result.folder}. ${plural(result.skipped.length, 'file')} could not be copied: their source can't be read.`
  } catch (err) {
    error.value = errorMessage(err)
  } finally {
    recovering.value = false
  }
}

async function copyList(): Promise<void> {
  if (!details.value) {
    return
  }
  try {
    await copyText(formatNotCopiedList(details.value))
    notice.value = `Copied the list of ${plural(details.value.notCopied.length, 'item')}. Paste it into an email or ticket.`
  } catch (err) {
    error.value = errorMessage(err)
  }
}

async function reveal(path: string): Promise<void> {
  try {
    await revealPath(path)
  } catch (err) {
    error.value = errorMessage(err)
  }
}

function sourcePath(relativePath: string): string {
  const source = summary.value?.settings.source ?? ''
  return `${source.replace(/[\\/]+$/, '')}\\${relativePath.replaceAll('/', '\\')}`
}
</script>

<template>
  <div class="mx-auto flex min-h-full max-w-6xl flex-col px-6 py-6 lg:px-10 lg:py-8">
    <RouterLink
      to="/compare/record"
      class="mb-3 inline-flex w-fit items-center gap-1.5 text-[13px] text-muted hover:text-fg"
    >
      <ArrowLeft class="size-3.5" />
      Record a copy
    </RouterLink>
    <PageHeader
      title="Record report"
      :summary="
        summary
          ? `${summary.settings.mode === 'watch' ? 'Watched' : 'Copied'} from ${summary.settings.source} to ${summary.settings.destination}.`
          : undefined
      "
    />

    <p
      v-if="error"
      class="mb-4 flex items-start gap-2 rounded-lg border border-danger/30 bg-danger-bg px-4 py-3 text-danger"
      role="alert"
    >
      <TriangleAlert class="mt-0.5 size-4 shrink-0" />
      {{ error }}
    </p>
    <p
      v-if="notice"
      class="mb-4 flex items-center gap-3 rounded-lg border border-stroke bg-card px-4 py-3"
      role="status"
    >
      <span class="flex-1">{{ notice }}</span>
      <button
        v-if="recoveryFolder"
        type="button"
        class="inline-flex h-7 items-center gap-1.5 rounded-md border border-stroke px-3 text-[13px] hover:bg-card-hover"
        @click="reveal(recoveryFolder)"
      >
        <FolderOpen class="size-3.5" />
        Show
      </button>
    </p>

    <template v-if="summary && totals">
      <div class="mb-3 flex justify-end gap-2">
        <button
          type="button"
          class="flex h-8 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover"
          title="Open in the browser to read, print or save as PDF"
          @click="openInBrowser"
        >
          <ExternalLink
            class="size-4"
            :stroke-width="1.75"
          />
          Open report
        </button>
        <button
          type="button"
          class="flex h-8 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover"
          title="Save as a web page for the client"
          @click="saveAs('html')"
        >
          <FileText
            class="size-4"
            :stroke-width="1.75"
          />
          Export HTML
        </button>
        <button
          type="button"
          class="flex h-8 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover"
          title="Save the not-copied list as CSV for Excel"
          @click="saveAs('csv')"
        >
          <FileSpreadsheet
            class="size-4"
            :stroke-width="1.75"
          />
          Export CSV
        </button>
      </div>
      <dl class="grid grid-cols-2 gap-3 tabular-nums lg:grid-cols-4">
        <div class="rounded-lg border border-stroke bg-card px-4 py-3">
          <dt class="text-[13px] text-muted">In the source</dt>
          <dd class="mt-1 text-xl font-semibold">{{ formatCount(totals.plannedFiles) }}</dd>
          <dd class="text-[13px] text-muted">files · {{ formatBytes(totals.plannedBytes) }}</dd>
        </div>
        <div class="rounded-lg border border-stroke bg-card px-4 py-3">
          <dt class="text-[13px] text-muted">
            {{ summary.settings.mode === 'watch' ? 'Arrived and checked' : 'Copied and checked' }}
          </dt>
          <dd class="mt-1 text-xl font-semibold text-success">{{ formatCount(totals.copied) }}</dd>
          <dd class="text-[13px] text-muted">{{ formatBytes(totals.copiedBytes) }}</dd>
        </div>
        <div class="rounded-lg border border-stroke bg-card px-4 py-3">
          <dt class="text-[13px] text-muted">Already there</dt>
          <dd class="mt-1 text-xl font-semibold">{{ formatCount(totals.skippedIdentical) }}</dd>
          <dd class="text-[13px] text-muted">same size and date</dd>
        </div>
        <div
          class="rounded-lg border px-4 py-3"
          :class="totals.notCopied > 0 ? 'border-danger/40 bg-danger-bg' : 'border-stroke bg-card'"
        >
          <dt class="text-[13px] text-muted">Not copied</dt>
          <dd
            class="mt-1 text-xl font-semibold"
            :class="totals.notCopied > 0 ? 'text-danger' : ''"
          >
            {{ formatCount(totals.notCopied) }}
          </dd>
          <dd class="text-[13px] text-muted">{{ formatBytes(totals.notCopiedBytes) }}</dd>
        </div>
      </dl>

      <p class="mt-3 text-[13px] text-muted">
        {{ stateText }} · started {{ when(summary.startedAtMs ?? summary.createdAtMs) }} · ended
        {{ when(summary.finishedAtMs) }} · {{ summary.machine }}
        <template v-if="summary.settings.verify === 'hash'"> · contents compared</template>
        <template v-if="summary.settings.ignoreJunk"> · system and temp files ignored</template>
        <template v-if="totals.excluded > 0">
          · {{ plural(totals.excluded, 'link') }} not followed</template
        >
      </p>
      <p
        v-if="summary.error"
        class="mt-2 text-danger"
      >
        {{ summary.error }}
      </p>

      <p
        v-if="finishedOk"
        class="mt-6 flex items-center gap-2 rounded-lg border border-stroke bg-card px-4 py-3"
      >
        <CircleCheck class="size-5 text-success" />
        Every file made it to the destination.
      </p>

      <section
        v-if="totals.notCopied > 0"
        class="mt-6 flex min-h-0 flex-1 flex-col"
      >
        <div class="mb-2 flex flex-wrap items-center gap-2">
          <h2 class="mr-auto font-semibold text-danger">
            {{ plural(totals.notCopied, 'item') }} did not make it
          </h2>
          <button
            type="button"
            class="inline-flex h-8 items-center gap-1.5 rounded-md bg-accent px-4 font-semibold text-on-accent hover:bg-accent-hover disabled:opacity-50"
            :disabled="busy"
            @click="retry()"
          >
            <RotateCcw class="size-4" />
            Retry missed
          </button>
          <button
            type="button"
            class="inline-flex h-8 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 hover:bg-card-hover disabled:opacity-50"
            :disabled="busy"
            @click="recover"
          >
            <FolderInput class="size-4" />
            Copy missed to…
          </button>
          <button
            type="button"
            class="inline-flex h-8 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 hover:bg-card-hover"
            @click="copyList"
          >
            <ClipboardList class="size-4" />
            Copy list
          </button>
        </div>

        <div
          v-if="job.running || recovering"
          class="mb-3 flex items-center gap-3 rounded-lg border border-stroke bg-card px-4 py-2 text-[13px]"
          aria-live="polite"
        >
          <LoaderCircle class="size-4 animate-spin text-accent" />
          <span class="flex-1 tabular-nums">
            <template v-if="recovering">Copying the missed files…</template>
            <template v-else-if="job.progress">
              Retrying {{ formatCount(job.progress.filesDone) }} of
              {{ formatCount(job.progress.filesTotal) }}
              <span
                v-if="job.progress.current"
                class="text-muted"
              >
                · {{ job.progress.current }}</span
              >
            </template>
            <template v-else>Retrying…</template>
          </span>
          <button
            v-if="job.running"
            type="button"
            class="h-7 rounded-md border border-stroke px-3 hover:bg-card-hover"
            @click="cancelRecord"
          >
            Cancel
          </button>
        </div>

        <ul class="mb-3 flex flex-wrap gap-2 text-[13px]">
          <li
            v-for="group in reasons"
            :key="group.reason"
          >
            <button
              type="button"
              class="rounded-full border border-danger/30 px-3 py-0.5 text-danger hover:bg-danger-bg disabled:opacity-50"
              :disabled="busy"
              :title="`Retry only: ${reasonTitle(group.reason)}`"
              @click="retry(group.reason)"
            >
              {{ reasonTitle(group.reason) }}: {{ formatCount(group.count) }}
              <RotateCcw class="mb-0.5 ml-1 inline size-3" />
            </button>
          </li>
        </ul>

        <div class="overflow-auto rounded-lg border border-stroke bg-card">
          <table class="w-full text-left text-[13px]">
            <thead class="sticky top-0 bg-card text-muted">
              <tr class="border-b border-stroke">
                <th class="px-3 py-2 font-normal">File or folder</th>
                <th class="px-3 py-2 text-right font-normal">Size</th>
                <th class="px-3 py-2 font-normal">Why</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="item in details?.notCopied.slice(0, shownRows)"
                :key="item.relativePath"
                class="border-b border-stroke last:border-0 hover:bg-card-hover"
                @dblclick="reveal(sourcePath(item.relativePath))"
              >
                <td class="max-w-0 px-3 py-1.5 text-danger">
                  <span
                    class="block truncate"
                    :title="item.relativePath"
                    >{{ item.relativePath.replaceAll('/', '\\')
                    }}{{ item.kind === 'folder' ? '\\' : '' }}</span
                  >
                </td>
                <td class="px-3 py-1.5 text-right whitespace-nowrap tabular-nums">
                  {{ item.kind === 'folder' ? '—' : formatBytes(item.size) }}
                </td>
                <td class="max-w-0 px-3 py-1.5">
                  <span
                    class="block truncate"
                    :title="item.message"
                  >
                    <span class="font-semibold text-danger">{{ reasonTitle(item.reason) }}</span>
                    <span
                      v-if="item.message"
                      class="text-muted"
                    >
                      · {{ item.message }}</span
                    >
                  </span>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <p
          v-if="totals.notCopied > shownRows"
          class="mt-2 text-[13px] text-muted"
        >
          Showing the first {{ formatCount(shownRows) }}. Retry and Copy missed to… cover all of
          them<template v-if="totals.notCopied > (details?.notCopied.length ?? 0)"
            >; Copy list covers the first
            {{ formatCount(details?.notCopied.length ?? 0) }}</template
          >.
        </p>
        <p class="mt-2 text-[13px] text-muted">
          Double-click a row to show the source file in Explorer. Click a reason above to retry only
          those files.
          <template v-if="summary.settings.mode === 'watch'">
            In Watch mode the reason is Deep Server Analyzer's best guess, since another program did
            the copy.
          </template>
        </p>
      </section>
    </template>
    <p
      v-else-if="!error"
      class="flex items-center gap-2 text-muted"
    >
      <LoaderCircle class="size-4 animate-spin" />
      Loading the report…
    </p>
  </div>
</template>
