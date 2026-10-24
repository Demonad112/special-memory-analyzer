<script setup lang="ts">
import { Copy, LoaderCircle, TriangleAlert } from '@lucide/vue'
import { computed, ref, watch } from 'vue'
import {
  allButOldest,
  cancelDuplicates,
  cleanupJob,
  DEFAULT_DUPE_MIN,
  DUPE_MIN_SIZES,
  DUPE_PAGE,
  dupeProgressLine,
  findDuplicates,
  groupsWithoutCopy,
  loadDuplicateGroups,
  loadDuplicateSummary,
  tickedDisk,
  type DupeGroup,
  type DupeProgress,
  type DupesSummary,
} from '@/lib/cleanup'
import { errorMessage } from '@/lib/errors'
import { formatBytes, formatDate, plural } from '@/lib/format'

const emit = defineEmits<{ review: [ids: number[]] }>()

const minSize = ref(DEFAULT_DUPE_MIN)
const running = ref(false)
const progress = ref<DupeProgress | null>(null)
const summary = ref<DupesSummary | null>(null)
const groups = ref<DupeGroup[]>([])
const error = ref('')
/** Nothing is ticked until the user ticks it or presses the button. */
const ticked = ref(new Set<number>())
/** Bumped by every search, reload and new scan, so a late answer writes nothing. */
let seq = 0

const noCopy = computed(() => groupsWithoutCopy(groups.value, ticked.value))
const tickedSize = computed(() => tickedDisk(groups.value, ticked.value))
const more = computed(() => (summary.value ? summary.value.groups > groups.value.length : false))

// A new scan drops the search in Rust; drop it here too.
watch(
  () => cleanupJob.finishedAt,
  () => {
    seq++
    running.value = false
    summary.value = null
    groups.value = []
    ticked.value = new Set()
    error.value = ''
  },
)

async function find(): Promise<void> {
  const mine = ++seq
  running.value = true
  progress.value = null
  error.value = ''
  summary.value = null
  groups.value = []
  ticked.value = new Set()
  try {
    const found = await findDuplicates(minSize.value, (next) => {
      if (mine === seq) {
        progress.value = next
      }
    })
    if (mine !== seq) {
      return
    }
    const first = await loadDuplicateGroups(0)
    if (mine === seq) {
      summary.value = found
      groups.value = first
    }
  } catch (err) {
    if (mine === seq && err !== 'cancelled') {
      error.value = errorMessage(err)
    }
  } finally {
    if (mine === seq) {
      running.value = false
    }
  }
}

function cancel(): void {
  void cancelDuplicates()
}

async function showMore(): Promise<void> {
  const mine = seq
  try {
    const next = await loadDuplicateGroups(groups.value.length)
    if (mine === seq) {
      groups.value = [...groups.value, ...next]
    }
  } catch (err) {
    if (mine === seq) {
      error.value = errorMessage(err)
    }
  }
}

function toggle(id: number): void {
  const next = new Set(ticked.value)
  if (!next.delete(id)) {
    next.add(id)
  }
  ticked.value = next
}

async function selectAllButOldest(): Promise<void> {
  const mine = seq
  try {
    const ids = await allButOldest()
    if (mine === seq) {
      ticked.value = new Set(ids)
    }
  } catch (err) {
    if (mine === seq) {
      error.value = errorMessage(err)
    }
  }
}

/** After a delete: the search in Rust has dropped what went; show what is left. */
async function reload(): Promise<void> {
  if (!summary.value) {
    return
  }
  const mine = ++seq
  const shown = Math.max(groups.value.length, DUPE_PAGE)
  try {
    const [left, totals] = await Promise.all([
      loadDuplicateGroups(0, shown),
      loadDuplicateSummary(),
    ])
    if (mine === seq) {
      const live = new Set(left.flatMap((group) => group.members.map((member) => member.id)))
      groups.value = left
      summary.value = totals
      ticked.value = new Set([...ticked.value].filter((id) => live.has(id)))
    }
  } catch (err) {
    if (mine === seq) {
      error.value = errorMessage(err)
    }
  }
}

defineExpose({ reload })
</script>

<template>
  <section
    class="rounded-lg border border-stroke bg-card p-4"
    aria-labelledby="dupes-title"
  >
    <div class="flex flex-wrap items-center gap-2">
      <Copy class="size-4 text-accent" />
      <h2
        id="dupes-title"
        class="font-display text-[15px] font-semibold"
      >
        Duplicates
      </h2>
      <span class="text-xs text-muted"
        >Files with identical content. Nothing is ticked for you.</span
      >
      <label class="ml-auto flex items-center gap-2 text-[13px]">
        At least
        <select
          v-model.number="minSize"
          class="h-8 rounded-md border border-stroke bg-app px-2"
          :disabled="running"
        >
          <option
            v-for="size in DUPE_MIN_SIZES"
            :key="size"
            :value="size"
          >
            {{ formatBytes(size) }}
          </option>
        </select>
      </label>
      <button
        v-if="!running"
        type="button"
        class="h-8 rounded-md border border-stroke-strong px-3 text-[13px] font-semibold hover:bg-subtle"
        @click="find"
      >
        Find duplicates
      </button>
      <button
        v-else
        type="button"
        class="h-8 rounded-md border border-stroke-strong px-3 text-[13px] font-semibold hover:bg-subtle"
        @click="cancel"
      >
        Cancel
      </button>
    </div>

    <p
      v-if="running"
      class="mt-3 flex items-center gap-2 text-[13px] text-muted"
      role="status"
    >
      <LoaderCircle class="size-4 animate-spin" />
      {{ dupeProgressLine(progress) }}
      <span
        v-if="progress && progress.bytes > 0"
        class="tabular-nums"
      >
        ({{ formatBytes(progress.bytes) }} read)
      </span>
    </p>
    <p
      v-else-if="error"
      class="mt-3 text-[13px] text-danger"
      role="alert"
    >
      {{ error }}
    </p>

    <template v-if="summary && !running">
      <p class="mt-3 text-[13px]">
        <template v-if="summary.groups === 0"
          >No duplicates of {{ formatBytes(summary.minSize) }} or more.</template
        >
        <template v-else>
          {{ plural(summary.groups, 'group') }}, {{ plural(summary.files, 'file') }}. Keeping one
          copy of each frees {{ formatBytes(summary.reclaimable) }}.
        </template>
      </p>
      <p
        v-if="summary.skipped.cloud + summary.skipped.hardLinked + summary.skipped.unreadable > 0"
        class="text-xs text-muted"
      >
        Not compared:
        <template v-if="summary.skipped.cloud > 0">
          {{ plural(summary.skipped.cloud, 'online-only file') }} (never downloaded);
        </template>
        <template v-if="summary.skipped.hardLinked > 0">
          {{ plural(summary.skipped.hardLinked, 'hard-linked name') }} (the same data, not a copy);
        </template>
        <template v-if="summary.skipped.unreadable > 0">
          {{ plural(summary.skipped.unreadable, 'file') }} that could not be read.
        </template>
      </p>

      <div
        v-if="groups.length > 0"
        class="mt-3 flex flex-wrap items-center gap-2"
      >
        <button
          type="button"
          class="h-8 rounded-md border border-stroke-strong px-3 text-[13px] hover:bg-subtle"
          @click="selectAllButOldest"
        >
          Select all but the oldest in each group
        </button>
        <button
          v-if="ticked.size > 0"
          type="button"
          class="h-8 rounded-md px-3 text-[13px] text-muted hover:bg-subtle"
          @click="ticked = new Set()"
        >
          Clear
        </button>
        <button
          type="button"
          class="ml-auto h-8 rounded-md bg-accent px-3 text-[13px] font-semibold text-on-accent hover:bg-accent-hover disabled:opacity-50"
          :disabled="ticked.size === 0 || noCopy.length > 0"
          @click="emit('review', [...ticked])"
        >
          Review {{ plural(ticked.size, 'file')
          }}<template v-if="ticked.size > 0"> · {{ formatBytes(tickedSize) }}</template>
        </button>
      </div>
      <p
        v-if="noCopy.length > 0"
        class="mt-2 flex items-center gap-2 text-[13px] text-warning"
        role="alert"
      >
        <TriangleAlert class="size-4 shrink-0" />
        Every copy is ticked in {{ plural(noCopy.length, 'group') }}. Keep at least one copy of
        each.
      </p>

      <ul class="mt-3 flex flex-col gap-3">
        <li
          v-for="group in groups"
          :key="group.id"
          class="border-t border-stroke pt-2"
        >
          <div class="flex items-baseline gap-2 text-[13px] font-semibold">
            <span
              class="min-w-0 flex-1 truncate"
              :title="group.members[0]?.name"
            >
              {{ group.members[0]?.name }}
            </span>
            <span class="shrink-0 font-normal text-muted tabular-nums">
              {{ group.members.length }} × {{ formatBytes(group.size) }} · frees
              {{ formatBytes(group.reclaimable) }}
            </span>
          </div>
          <ul class="text-[13px]">
            <li
              v-for="(member, at) in group.members"
              :key="member.id"
            >
              <label class="flex items-baseline gap-2 py-0.5">
                <input
                  type="checkbox"
                  :checked="ticked.has(member.id)"
                  @change="toggle(member.id)"
                />
                <span
                  class="min-w-0 flex-1 truncate"
                  :title="`${member.folder}\\${member.name}`"
                >
                  {{ member.folder }}
                  <span
                    v-if="member.name !== group.members[0]?.name"
                    class="text-muted"
                  >
                    · {{ member.name }}
                  </span>
                </span>
                <span
                  v-if="at === 0"
                  class="shrink-0 text-xs text-muted"
                >
                  Oldest
                </span>
                <span class="w-24 shrink-0 text-right text-xs text-muted tabular-nums">
                  {{ member.modifiedMs === null ? '' : formatDate(member.modifiedMs) }}
                </span>
              </label>
            </li>
          </ul>
        </li>
      </ul>
      <button
        v-if="more"
        type="button"
        class="mt-3 h-8 rounded-md border border-stroke-strong px-3 text-[13px] hover:bg-subtle"
        @click="showMore"
      >
        Show more groups
      </button>
    </template>
  </section>
</template>
