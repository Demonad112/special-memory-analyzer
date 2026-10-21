<script setup lang="ts">
import { ArrowLeft, ArrowLeftRight, Copy, Eye, LoaderCircle, TriangleAlert } from '@lucide/vue'
import { computed, onBeforeUnmount, onMounted, reactive, ref } from 'vue'
import { useRouter } from 'vue-router'
import PageHeader from '@/components/PageHeader.vue'
import JobFields from '@/components/JobFields.vue'
import ShareSignIn from '@/components/ShareSignIn.vue'
import PathField from '@/components/PathField.vue'
import { errorMessage, ignoreJunk, loadPaths, pickFolder, savePaths } from '@/lib/compare'
import { formatBytes, formatCount, plural } from '@/lib/format'
import {
  bytesPerSecond,
  cancelRecord,
  finishRecord,
  formatTimeLeft,
  conflictChoices,
  defaultPreserve,
  isCancelled,
  listRecords,
  recordJob,
  startRecord,
  timeLeftMs,
  type ConflictChoice,
  type PreserveOptions,
  type RecordMode,
  type RecordProgress,
  type RunSummary,
} from '@/lib/record'

const router = useRouter()
const paths = reactive(loadPaths())
const mode = ref<RecordMode>('watch')
const hash = ref(false)
const downloadCloud = ref(false)
// Not remembered between runs: replacing files is a choice to make each time.
const conflict = ref<ConflictChoice>('skip')
const keep = reactive<PreserveOptions>({ ...defaultPreserve })
const keepChoices: { key: keyof PreserveOptions; label: string }[] = [
  { key: 'timestamps', label: 'Keep dates (created, modified, accessed; folders too)' },
  { key: 'attributes', label: 'Keep attributes (read-only, hidden, system, archive)' },
  { key: 'acl', label: 'Keep permissions (who can open it)' },
  { key: 'owner', label: 'Keep owner (needs Administrator to set someone else)' },
  { key: 'audit', label: 'Keep audit rules (needs Administrator)' },
  {
    key: 'streams',
    label: 'Keep alternate data streams (e.g. the "downloaded from the internet" mark)',
  },
]
const error = ref('')
const recent = ref<RunSummary[]>([])

const job = recordJob
const canStart = computed(
  () => !job.running && paths.left.trim() !== '' && paths.right.trim() !== '',
)

// Re-render the speed and time left once a second even when no progress arrives.
const now = ref(Date.now())
let ticker: ReturnType<typeof setInterval> | undefined
onMounted(async () => {
  ticker = setInterval(() => (now.value = Date.now()), 1000)
  try {
    recent.value = (await listRecords()).slice(0, 5)
  } catch {
    // No records yet, or outside the desktop app.
  }
})
onBeforeUnmount(() => clearInterval(ticker))

const modes: { value: RecordMode; title: string; text: string; icon: typeof Eye }[] = [
  {
    value: 'watch',
    title: 'Watch',
    text: 'Another program copies (Explorer, robocopy, a backup tool). Deep Server Analyzer checks every file that arrives.',
    icon: Eye,
  },
  {
    value: 'copy',
    title: 'Copy for me',
    text: 'Deep Server Analyzer copies the files itself, retries the ones in use, and checks each copy.',
    icon: Copy,
  },
]

async function browse(side: 'left' | 'right'): Promise<void> {
  const picked = await pickFolder(
    side === 'left' ? 'Copied from' : 'Copied to',
    paths[side] || undefined,
  )
  if (picked) {
    paths[side] = picked
  }
}

function swap(): void {
  ;[paths.left, paths.right] = [paths.right, paths.left]
}

async function start(): Promise<void> {
  if (!canStart.value) {
    return
  }
  savePaths(paths)
  error.value = ''
  job.running = true
  job.mode = mode.value
  job.progress = null
  job.copyStartedAt = 0
  job.finishing = false
  try {
    const details = await startRecord(
      paths.left,
      paths.right,
      mode.value,
      hash.value ? 'hash' : 'sizeAndTime',
      {
        ignoreJunk: ignoreJunk.value,
        downloadCloud: mode.value === 'copy' && downloadCloud.value,
        preserve: { ...keep },
        conflict: mode.value === 'copy' ? conflict.value : undefined,
      },
      (progress: RecordProgress) => {
        // "Done" closes each step (listing, then the copy); the next step's progress follows.
        if (progress.phase === 'done') {
          return
        }
        if (progress.phase !== 'scanning' && job.copyStartedAt === 0) {
          job.copyStartedAt = Date.now()
        }
        job.progress = progress
      },
    )
    await router.push(`/compare/record/${details.summary.id}`)
  } catch (err) {
    if (!isCancelled(err)) {
      error.value = errorMessage(err)
    }
  } finally {
    job.running = false
  }
}

async function finish(): Promise<void> {
  job.finishing = true
  await finishRecord()
}

async function cancel(): Promise<void> {
  await cancelRecord()
}

const progress = computed(() => job.progress)
const percent = computed(() => {
  const value = progress.value
  if (!value || value.phase === 'scanning' || value.bytesTotal === 0) {
    return null
  }
  return Math.min(100, (value.bytesDone / value.bytesTotal) * 100)
})
const speedText = computed(() => {
  const value = progress.value
  if (!value || job.copyStartedAt === 0) {
    return ''
  }
  const elapsed = now.value - job.copyStartedAt
  const speed = bytesPerSecond(value.bytesDone, elapsed)
  const left = timeLeftMs(value.bytesDone, value.bytesTotal, elapsed)
  const parts = speed > 0 ? [`${formatBytes(speed)}/s`] : []
  if (left !== null && job.mode === 'copy') {
    parts.push(formatTimeLeft(left))
  }
  return parts.join(' · ')
})
const phaseText = computed(() => {
  switch (progress.value?.phase) {
    case undefined:
    case 'scanning':
      return 'Listing the source…'
    case 'copying':
      return 'Copying'
    case 'watching':
      return 'Watching the destination'
    case 'finishing':
      return 'Checking what did not arrive…'
    case 'done':
      return 'Finishing…'
  }
})

function stateText(run: RunSummary): string {
  if (run.state === 'completed') {
    return run.totals.notCopied === 0
      ? 'All copied'
      : `${plural(run.totals.notCopied, 'item')} not copied`
  }
  return run.state.charAt(0).toUpperCase() + run.state.slice(1)
}

function when(ms: number): string {
  return new Date(ms).toLocaleString('en-US', { dateStyle: 'medium', timeStyle: 'short' })
}
</script>

<template>
  <div class="mx-auto flex min-h-full max-w-6xl flex-col px-6 py-6 lg:px-10 lg:py-8">
    <RouterLink
      to="/compare"
      class="mb-3 inline-flex w-fit items-center gap-1.5 text-[13px] text-muted hover:text-fg"
    >
      <ArrowLeft class="size-3.5" />
      Compare
    </RouterLink>
    <div class="flex items-start justify-between gap-6">
      <PageHeader
        title="Record a copy"
        summary="Record a copy from the source to the destination, then see every file that did not make it and why."
      />
      <button
        v-if="!job.running"
        type="button"
        class="mt-1.5 h-8 shrink-0 rounded-md bg-accent px-5 font-semibold text-on-accent hover:bg-accent-hover disabled:opacity-50"
        :disabled="!canStart"
        @click="start"
      >
        {{ mode === 'watch' ? 'Start watching' : 'Start copy' }}
      </button>
    </div>

    <div class="grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-center gap-2">
      <PathField
        v-model="paths.left"
        label="Source"
        hint="Copied from"
        :disabled="job.running"
        @browse="browse('left')"
        @submit="start"
      />
      <button
        type="button"
        class="grid size-8 place-items-center rounded-md text-muted hover:bg-subtle-strong disabled:opacity-50"
        title="Swap source and destination"
        aria-label="Swap source and destination"
        :disabled="job.running"
        @click="swap"
      >
        <ArrowLeftRight class="size-4" />
      </button>
      <PathField
        v-model="paths.right"
        label="Destination"
        hint="Copied to"
        :disabled="job.running"
        @browse="browse('right')"
        @submit="start"
      />
    </div>

    <p
      v-if="error"
      class="mt-4 flex items-start gap-2 rounded-lg border border-danger/30 bg-danger-bg px-4 py-3 text-danger"
      role="alert"
    >
      <TriangleAlert class="mt-0.5 size-4 shrink-0" />
      {{ error }}
    </p>

    <section
      v-if="!job.running"
      class="mt-5"
      aria-label="How the files get copied"
    >
      <div
        class="grid gap-3 sm:grid-cols-2"
        role="radiogroup"
        aria-label="Mode"
      >
        <button
          v-for="option in modes"
          :key="option.value"
          type="button"
          role="radio"
          :aria-checked="mode === option.value"
          class="flex gap-3 rounded-lg border bg-card px-4 py-3 text-left hover:bg-card-hover"
          :class="mode === option.value ? 'border-accent ring-1 ring-accent' : 'border-stroke'"
          @click="mode = option.value"
        >
          <component
            :is="option.icon"
            class="mt-0.5 size-5 shrink-0 text-accent"
            :stroke-width="1.75"
          />
          <span>
            <span class="block font-semibold">{{ option.title }}</span>
            <span class="mt-0.5 block text-[13px] text-muted">{{ option.text }}</span>
          </span>
        </button>
      </div>
      <label class="mt-3 flex w-fit items-center gap-2 text-[13px]">
        <input
          v-model="hash"
          type="checkbox"
          class="size-4 accent-accent"
        />
        Also compare file contents (slower: every file is read twice)
      </label>
      <label class="mt-2 flex w-fit items-center gap-2 text-[13px]">
        <input
          v-model="ignoreJunk"
          type="checkbox"
          class="size-4 accent-accent"
        />
        Ignore system and temp files (Thumbs.db, desktop.ini, ~$ Office lock files, .tmp)
      </label>
      <label
        v-if="mode === 'copy'"
        class="mt-2 flex w-fit items-center gap-2 text-[13px]"
      >
        <input
          v-model="downloadCloud"
          type="checkbox"
          class="size-4 accent-accent"
        />
        Download OneDrive online-only files (off: they are listed as not copied)
      </label>
      <label
        v-if="mode === 'copy'"
        class="mt-2 flex w-fit items-center gap-2 text-[13px]"
      >
        If a different file is already at the destination:
        <select
          v-model="conflict"
          class="h-8 rounded-md border border-stroke border-b-stroke-strong bg-card px-2 text-[13px] outline-none focus:border-b-2 focus:border-b-accent"
        >
          <option
            v-for="choice in conflictChoices"
            :key="choice.value"
            :value="choice.value"
          >
            {{ choice.label }}
          </option>
        </select>
      </label>
      <template v-if="mode === 'copy'">
        <label
          v-for="choice in keepChoices"
          :key="choice.key"
          class="mt-2 flex w-fit items-center gap-2 text-[13px]"
        >
          <input
            v-model="keep[choice.key]"
            type="checkbox"
            class="size-4 accent-accent"
          />
          {{ choice.label }}
        </label>
      </template>
      <ShareSignIn :paths="[paths.left, paths.right]" />
      <JobFields class="mt-4" />
      <p
        v-if="mode === 'watch'"
        class="mt-3 text-[13px] text-muted"
      >
        Start watching first, then start the copy in the other program. When it is done, press
        <strong class="font-semibold text-fg">Finish and check</strong>.
      </p>

      <div
        v-if="recent.length > 0"
        class="mt-8"
      >
        <h2 class="mb-2 font-semibold">Recent records</h2>
        <ul class="divide-y divide-stroke overflow-hidden rounded-lg border border-stroke bg-card">
          <li
            v-for="run in recent"
            :key="run.id"
          >
            <RouterLink
              :to="`/compare/record/${run.id}`"
              class="flex items-center gap-4 px-4 py-2.5 hover:bg-card-hover"
            >
              <span class="min-w-0 flex-1">
                <span class="block truncate">{{ run.settings.source }}</span>
                <span class="block truncate text-[13px] text-muted">
                  to {{ run.settings.destination }}
                </span>
              </span>
              <span class="shrink-0 text-right text-[13px]">
                <span
                  class="block"
                  :class="run.totals.notCopied > 0 ? 'text-danger' : 'text-success'"
                  >{{ stateText(run) }}</span
                >
                <span class="block text-muted">{{ when(run.createdAtMs) }}</span>
              </span>
            </RouterLink>
          </li>
        </ul>
      </div>
    </section>

    <section
      v-else
      class="mt-5 rounded-lg border border-stroke bg-card px-5 py-4"
      aria-live="polite"
    >
      <div class="flex items-center gap-2 font-semibold">
        <LoaderCircle class="size-4 animate-spin text-accent" />
        {{ phaseText }}
      </div>
      <div
        class="mt-3 h-1.5 overflow-hidden rounded-full bg-subtle-strong"
        role="progressbar"
        :aria-valuenow="percent ?? undefined"
        aria-valuemin="0"
        aria-valuemax="100"
      >
        <div
          class="h-full rounded-full bg-accent transition-[width] duration-200"
          :class="{ 'w-1/3 animate-pulse': percent === null }"
          :style="percent === null ? undefined : { width: `${String(percent)}%` }"
        />
      </div>
      <dl
        v-if="progress"
        class="mt-3 grid grid-cols-2 gap-x-6 gap-y-1 text-[13px] tabular-nums sm:grid-cols-4"
      >
        <div>
          <dt class="text-muted">Files</dt>
          <dd>
            <template v-if="progress.phase === 'scanning'"
              >{{ formatCount(progress.filesDone) }} found</template
            >
            <template v-else>
              {{ formatCount(progress.filesDone) }} of {{ formatCount(progress.filesTotal) }}
            </template>
          </dd>
        </div>
        <div>
          <dt class="text-muted">Size</dt>
          <dd>
            <template v-if="progress.phase === 'scanning'">{{
              formatBytes(progress.bytesTotal)
            }}</template>
            <template v-else>
              {{ formatBytes(progress.bytesDone) }} of {{ formatBytes(progress.bytesTotal) }}
            </template>
          </dd>
        </div>
        <div>
          <dt class="text-muted">Speed</dt>
          <dd>{{ speedText || '—' }}</dd>
        </div>
        <div>
          <dt class="text-muted">Not copied</dt>
          <dd :class="progress.notCopied > 0 ? 'font-semibold text-danger' : ''">
            {{ formatCount(progress.notCopied) }}
          </dd>
        </div>
      </dl>
      <p
        v-if="progress?.current"
        class="mt-2 truncate text-[13px] text-muted"
        :title="progress.current"
      >
        {{ progress.current }}
      </p>
      <div class="mt-4 flex gap-2">
        <button
          v-if="job.mode === 'watch'"
          type="button"
          class="h-8 rounded-md bg-accent px-4 font-semibold text-on-accent hover:bg-accent-hover disabled:opacity-50"
          :disabled="job.finishing || progress?.phase === 'scanning'"
          @click="finish"
        >
          Finish and check
        </button>
        <button
          type="button"
          class="h-8 rounded-md border border-stroke bg-card px-4 hover:bg-card-hover"
          @click="cancel"
        >
          Cancel
        </button>
      </div>
    </section>
  </div>
</template>
