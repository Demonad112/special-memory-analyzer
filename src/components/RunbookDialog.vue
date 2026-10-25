<script setup lang="ts">
import { LoaderCircle } from '@lucide/vue'
import { computed, reactive, ref } from 'vue'
import { errorMessage } from '@/lib/errors'
import { formatDateTime } from '@/lib/format'
import { jobLabel, type JobInfo } from '@/lib/job'
import {
  createRunbook,
  kindLabels,
  runbookCandidates,
  runbookJob,
  runbookProblem,
  type ReportEntry,
} from '@/lib/reports'

const props = defineProps<{ entries: ReportEntry[] }>()
const emit = defineEmits<{ created: [entry: ReportEntry] }>()

const el = ref<HTMLDialogElement | null>(null)
/** `kind:id` of the picked reports, in the order they were ticked: the first one sets the ticket. */
const picked = ref<string[]>([])
const info = reactive<JobInfo>({ client: '', ticket: '', technician: '' })
/** The job fields were typed in; stop following the first pick. */
const typed = ref(false)
const busy = ref(false)
const error = ref('')
/** Bumped when the dialog opens or closes, so a late answer from Rust writes nothing. */
let seq = 0

const keyOf = (entry: ReportEntry): string => `${entry.kind}:${entry.id}`

const first = computed(() => props.entries.find((entry) => keyOf(entry) === picked.value[0]))
const rows = computed(() => runbookCandidates(props.entries, first.value ?? null))
const chosen = computed(() =>
  picked.value.flatMap((key) => props.entries.filter((entry) => keyOf(entry) === key)),
)
const problem = computed(() => runbookProblem(chosen.value))

function setJob(source: JobInfo): void {
  info.client = source.client
  info.ticket = source.ticket
  info.technician = source.technician
}

function toggle(entry: ReportEntry): void {
  const key = keyOf(entry)
  const wasFirst = picked.value[0] === key
  picked.value = picked.value.includes(key)
    ? picked.value.filter((candidate) => candidate !== key)
    : [...picked.value, key]
  if (!typed.value && (wasFirst || picked.value.length === 1)) {
    setJob(runbookJob(first.value))
  }
}

function open(): void {
  seq += 1
  picked.value = []
  typed.value = false
  busy.value = false
  error.value = ''
  setJob(runbookJob(undefined))
  el.value?.showModal()
}

function close(): void {
  seq += 1
  el.value?.close()
}

async function make(): Promise<void> {
  if (problem.value || busy.value) {
    return
  }
  const mine = ++seq
  busy.value = true
  error.value = ''
  try {
    const made = await createRunbook(
      {
        client: info.client.trim(),
        ticket: info.ticket.trim(),
        technician: info.technician.trim(),
      },
      chosen.value,
    )
    if (mine !== seq) {
      return
    }
    el.value?.close()
    emit('created', made)
  } catch (err) {
    if (mine === seq) {
      error.value = errorMessage(err)
    }
  } finally {
    if (mine === seq) {
      busy.value = false
    }
  }
}

defineExpose({ open })
</script>

<template>
  <dialog
    ref="el"
    class="m-auto w-[min(40rem,92vw)] rounded-lg border border-stroke-strong bg-app p-5 text-fg shadow-xl backdrop:bg-fg/20"
    aria-labelledby="runbook-title"
    @close="seq += 1"
  >
    <h2
      id="runbook-title"
      class="font-display text-lg font-semibold"
    >
      New runbook
    </h2>
    <p class="mt-1 text-[13px] text-muted">
      One document for a migration: the readiness report, the copy runs and the compare reports. It
      keeps the reports it is made from; deleting it deletes none of them.
    </p>

    <ul
      class="mt-3 max-h-72 overflow-y-auto rounded-lg border border-stroke bg-card"
      aria-label="Reports to include"
    >
      <li
        v-for="row in rows"
        :key="keyOf(row.entry)"
        class="border-b border-stroke last:border-b-0"
      >
        <label class="flex cursor-pointer items-start gap-3 px-3 py-2 hover:bg-subtle">
          <input
            type="checkbox"
            class="mt-1"
            :checked="picked.includes(keyOf(row.entry))"
            @change="toggle(row.entry)"
          />
          <span class="min-w-0 flex-1">
            <span class="flex items-baseline gap-2 text-[13px]">
              <span class="font-semibold">{{ row.entry.title }}</span>
              <span
                v-if="jobLabel(row.entry.job)"
                class="truncate text-xs text-accent"
              >
                {{ jobLabel(row.entry.job) }}
              </span>
              <span
                v-if="row.suggested && !picked.includes(keyOf(row.entry))"
                class="shrink-0 rounded bg-subtle-strong px-1.5 text-xs"
              >
                Same ticket
              </span>
            </span>
            <span
              class="block truncate text-xs text-muted"
              :title="row.entry.subject"
            >
              {{ row.entry.subject }}
            </span>
            <span class="block text-xs text-faint tabular-nums">
              {{ kindLabels[row.entry.kind] }} · {{ formatDateTime(row.entry.createdAtMs) }} ·
              {{ row.entry.headline }}
            </span>
          </span>
        </label>
      </li>
      <li
        v-if="rows.length === 0"
        class="px-3 py-6 text-center text-[13px] text-muted"
      >
        No readiness, record or compare reports yet.
      </li>
    </ul>

    <div class="mt-3 grid grid-cols-3 gap-2 text-[13px]">
      <label class="block">
        <span class="text-xs text-muted">Client</span>
        <input
          v-model="info.client"
          type="text"
          class="mt-0.5 h-8 w-full rounded-md border border-stroke border-b-stroke-strong bg-card px-2 outline-none focus:border-b-2 focus:border-b-accent"
          @input="typed = true"
        />
      </label>
      <label class="block">
        <span class="text-xs text-muted">Ticket</span>
        <input
          v-model="info.ticket"
          type="text"
          class="mt-0.5 h-8 w-full rounded-md border border-stroke border-b-stroke-strong bg-card px-2 outline-none focus:border-b-2 focus:border-b-accent"
          @input="typed = true"
        />
      </label>
      <label class="block">
        <span class="text-xs text-muted">Technician</span>
        <input
          v-model="info.technician"
          type="text"
          class="mt-0.5 h-8 w-full rounded-md border border-stroke border-b-stroke-strong bg-card px-2 outline-none focus:border-b-2 focus:border-b-accent"
          @input="typed = true"
        />
      </label>
    </div>

    <p
      v-if="error"
      class="mt-3 rounded-md border border-danger/40 bg-danger-bg px-3 py-2 text-[13px] text-danger"
      role="alert"
    >
      {{ error }}
    </p>
    <div class="mt-4 flex items-center justify-end gap-2">
      <span
        v-if="picked.length > 0 && problem"
        class="mr-auto text-xs text-danger"
      >
        {{ problem }}
      </span>
      <span
        v-else
        class="mr-auto text-xs text-muted"
      >
        {{ picked.length }} picked
      </span>
      <button
        type="button"
        class="h-8 rounded-md border border-stroke bg-card px-4 hover:bg-card-hover"
        @click="close"
      >
        Cancel
      </button>
      <button
        type="button"
        class="flex h-8 items-center gap-1.5 rounded-md bg-accent px-4 font-semibold text-on-accent hover:bg-accent-hover disabled:opacity-50"
        :disabled="problem !== '' || busy"
        @click="make"
      >
        <LoaderCircle
          v-if="busy"
          class="size-4 animate-spin"
        />
        Make runbook
      </button>
    </div>
  </dialog>
</template>
