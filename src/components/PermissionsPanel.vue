<script setup lang="ts">
import { Download, LoaderCircle, ShieldAlert, ShieldCheck } from '@lucide/vue'
import { computed, ref, watch } from 'vue'
import { errorMessage } from '@/lib/errors'
import { plural } from '@/lib/format'
import {
  findingHeadings,
  findingHints,
  issueKinds,
  loadFindings,
  type Finding,
  type FindingKind,
  type PermissionsSummary,
} from '@/lib/readiness'
import { currentJob } from '@/lib/job'
import { exportReport } from '@/lib/reports'

const props = defineProps<{
  permissions: PermissionsSummary
  /** The saved report the CSV is written from; `null` when it could not be saved. */
  reportId: string | null
  /** Changes with every new check: drops what was loaded for the last one. */
  view: number
}>()

const PAGE = 200
const kinds: FindingKind[] = [
  'brokenInheritance',
  'unknownAccount',
  'wideWrite',
  'explicitFile',
  'unreadable',
]

const counts = computed(() =>
  kinds
    .map((kind) => ({
      kind,
      count: props.permissions.counts.find((entry) => entry.kind === kind)?.count ?? 0,
    }))
    .filter((entry) => entry.count > 0),
)
const checked = computed(() => props.permissions.status === 'checked')

const open = ref<FindingKind | null>(null)
const findings = ref<Finding[]>([])
const total = ref(0)
const loading = ref(false)
const error = ref('')
const notice = ref('')
/** Bumped by every load and every new check, so a late page writes nothing. */
let seq = 0

async function show(kind: FindingKind): Promise<void> {
  if (open.value === kind) {
    seq++
    open.value = null
    return
  }
  const mine = ++seq
  open.value = kind
  findings.value = []
  total.value = 0
  await more(mine, kind)
}

async function more(mine: number, kind: FindingKind): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    const page = await loadFindings(kind, findings.value.length, PAGE)
    if (mine === seq) {
      findings.value = [...findings.value, ...page.findings]
      total.value = page.total
    }
  } catch (err) {
    if (mine === seq) {
      error.value = errorMessage(err)
    }
  } finally {
    if (mine === seq) {
      loading.value = false
    }
  }
}

function loadMore(): void {
  if (open.value && !loading.value) {
    void more(seq, open.value)
  }
}

watch(
  () => props.view,
  () => {
    seq++
    exportSeq++
    exporting.value = false
    open.value = null
    findings.value = []
    notice.value = ''
    error.value = ''
    loading.value = false
  },
)

const exporting = ref(false)
/** Bumped by every new check, so an export that finishes after it writes nothing. */
let exportSeq = 0

async function exportCsv(): Promise<void> {
  if (!props.reportId || exporting.value) {
    return
  }
  const mine = exportSeq
  exporting.value = true
  error.value = ''
  notice.value = ''
  try {
    const path = await exportReport(
      {
        kind: 'readiness',
        id: props.reportId,
        title: 'Migration readiness',
        createdAtMs: Date.now(),
        job: currentJob(),
      },
      'csv',
      'permissions',
    )
    if (path && mine === exportSeq) {
      notice.value = `Saved ${path}`
    }
  } catch (err) {
    if (mine === exportSeq) {
      error.value = errorMessage(err)
    }
  } finally {
    if (mine === exportSeq) {
      exporting.value = false
    }
  }
}

function where(finding: Finding): string {
  if (!finding.path) {
    return '(the source folder)'
  }
  return finding.dir ? `${finding.path}\\` : finding.path
}
</script>

<template>
  <section
    class="mt-4 rounded-lg border border-stroke bg-card"
    aria-labelledby="permissions-heading"
  >
    <div class="flex flex-wrap items-center gap-3 px-4 py-3">
      <component
        :is="permissions.issues > 0 ? ShieldAlert : ShieldCheck"
        class="size-4 shrink-0"
        :class="permissions.issues > 0 ? 'text-warning' : 'text-success'"
      />
      <h2
        id="permissions-heading"
        class="font-semibold"
      >
        Permissions
      </h2>
      <span
        v-if="checked"
        class="text-[13px] text-muted"
      >
        {{ plural(permissions.foldersRead, 'folder') }}
        <template v-if="permissions.everyFile">
          and {{ plural(permissions.filesRead, 'file') }}
        </template>
        read, nothing changed ·
        {{ permissions.elevated ? 'as administrator' : 'without administrator rights' }}
        <template v-if="permissions.server"> · names asked of {{ permissions.server }}</template>
        <template v-if="permissions.serverNote"> · {{ permissions.serverNote }}</template>
      </span>
      <span
        v-else
        class="text-[13px] text-muted"
      >
        {{
          permissions.status === 'notSupported'
            ? 'Not checked on this system.'
            : 'Not checked (turned off for this check).'
        }}
      </span>
      <span class="flex-1" />
      <button
        v-if="checked"
        type="button"
        class="flex h-8 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover disabled:opacity-50"
        :disabled="!reportId || exporting"
        :title="reportId ? '' : 'The report was not saved, so there is nothing to export.'"
        @click="exportCsv"
      >
        <LoaderCircle
          v-if="exporting"
          class="size-4 animate-spin"
        />
        <Download
          v-else
          class="size-4"
        />
        Export permissions (CSV)
      </button>
    </div>

    <template v-if="checked">
      <p
        v-if="counts.length === 0"
        class="border-t border-stroke px-4 py-3 text-[13px] text-success"
      >
        Nothing to report: no broken inheritance, unknown accounts or wide write access.
      </p>
      <div
        v-else
        class="flex flex-wrap gap-2 border-t border-stroke px-4 py-3"
      >
        <button
          v-for="entry in counts"
          :key="entry.kind"
          type="button"
          class="h-8 rounded-md border px-3 text-[13px]"
          :class="
            open === entry.kind
              ? 'border-accent bg-accent/12'
              : 'border-stroke bg-card hover:bg-card-hover'
          "
          :aria-pressed="open === entry.kind"
          @click="show(entry.kind)"
        >
          <span :class="issueKinds.includes(entry.kind) ? 'text-warning' : ''">{{
            findingHeadings[entry.kind]
          }}</span>
          · {{ entry.count.toLocaleString() }}
        </button>
      </div>
      <p
        v-if="!permissions.everyFile"
        class="border-t border-stroke px-4 py-2 text-xs text-muted"
      >
        Folders only. Turn on Check every file to find files with entries of their own.
      </p>

      <div
        v-if="open"
        class="border-t border-stroke"
      >
        <p class="px-4 pt-3 text-xs text-muted">{{ findingHints[open] }}</p>
        <ul class="max-h-96 overflow-y-auto py-2">
          <li
            v-for="(finding, index) in findings"
            :key="`${index}-${finding.path}`"
            class="px-4 py-1.5 text-[13px]"
          >
            <span class="break-all">{{ where(finding) }}</span>
            <span class="text-muted"> — {{ finding.detail }}</span>
          </li>
        </ul>
        <div class="flex items-center gap-3 px-4 pb-3 text-xs text-muted">
          <LoaderCircle
            v-if="loading"
            class="size-3.5 animate-spin"
          />
          <span class="tabular-nums">
            {{ findings.length.toLocaleString() }} of {{ total.toLocaleString() }} shown
          </span>
          <button
            v-if="findings.length < total && !loading"
            type="button"
            class="rounded border border-stroke px-2 py-0.5 hover:bg-card-hover"
            @click="loadMore"
          >
            Show more
          </button>
        </div>
      </div>
    </template>

    <p
      v-if="permissions.itemsTruncated"
      class="border-t border-stroke px-4 py-2 text-xs text-warning"
    >
      The CSV stops at the first 100,000 access entries. Run the check on a smaller folder for the
      rest.
    </p>
    <p
      v-if="error"
      class="border-t border-stroke px-4 py-2 text-[13px] text-danger"
      role="alert"
    >
      {{ error }}
    </p>
    <p
      v-if="notice"
      class="border-t border-stroke px-4 py-2 text-[13px] text-muted"
      role="status"
    >
      {{ notice }}
    </p>
  </section>
</template>
