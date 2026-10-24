<script setup lang="ts">
import { Funnel, FunnelX, LoaderCircle, Users } from '@lucide/vue'
import { ref, watch } from 'vue'
import {
  cancelOwners,
  cleanupJob,
  loadOwnerTable,
  ownerProgressLine,
  readOwners,
  setOwnerFilter,
  type OwnerProgress,
  type OwnerRow,
  type OwnerTable,
} from '@/lib/cleanup'
import { errorMessage } from '@/lib/errors'
import { formatBytes, percentShare, plural } from '@/lib/format'

const table = ref<OwnerTable | null>(null)
const running = ref(false)
const progress = ref<OwnerProgress | null>(null)
const error = ref('')
/** Bumped by every pass, reload and new scan, so a late answer writes nothing. */
let seq = 0

// A new scan drops the owners in Rust (D-15); drop them here too, and the filter with them.
watch(
  () => cleanupJob.finishedAt,
  () => {
    seq++
    running.value = false
    table.value = null
    error.value = ''
  },
)

async function read(): Promise<void> {
  const mine = ++seq
  running.value = true
  progress.value = null
  error.value = ''
  table.value = null
  try {
    const next = await readOwners((update) => {
      if (mine === seq) {
        progress.value = update
      }
    })
    if (mine === seq) {
      table.value = next
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
  void cancelOwners()
}

async function filterBy(row: OwnerRow): Promise<void> {
  const same = cleanupJob.ownerFilter?.index === row.index
  try {
    await setOwnerFilter(same ? null : { index: row.index, name: row.name })
  } catch (err) {
    error.value = errorMessage(err)
  }
}

/** After a delete: the owners in Rust are the same; show the totals for what is left. */
async function reload(): Promise<void> {
  if (!table.value) {
    return
  }
  const mine = ++seq
  try {
    const next = await loadOwnerTable()
    if (mine === seq) {
      table.value = next
      const filter = cleanupJob.ownerFilter
      if (filter && !next?.rows.some((row) => row.index === filter.index)) {
        await setOwnerFilter(null)
      }
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
    aria-labelledby="owners-title"
  >
    <div class="flex flex-wrap items-center gap-2">
      <Users class="size-4 text-accent" />
      <h2
        id="owners-title"
        class="font-display text-[15px] font-semibold"
      >
        Space by owner
      </h2>
      <button
        v-if="!running"
        type="button"
        class="ml-auto h-8 rounded-md border border-stroke bg-app px-3 text-[13px] hover:bg-card-hover"
        @click="read"
      >
        {{ table ? 'Read again' : 'By owner' }}
      </button>
      <button
        v-else
        type="button"
        class="ml-auto h-8 rounded-md border border-stroke bg-app px-3 text-[13px] hover:bg-card-hover"
        @click="cancel"
      >
        Cancel
      </button>
    </div>
    <p
      v-if="!table && !running && !error"
      class="mt-2 text-xs text-muted"
    >
      Reads who owns each file. It takes a moment on a big scan, so it only runs when you ask.
    </p>
    <p
      v-if="running"
      class="mt-3 flex items-center gap-2 text-[13px] text-muted"
      role="status"
    >
      <LoaderCircle class="size-4 animate-spin" />
      {{ ownerProgressLine(progress) }}
    </p>
    <p
      v-if="error"
      class="mt-3 text-[13px] text-danger"
      role="alert"
    >
      {{ error }}
    </p>

    <template v-if="table && !running">
      <p
        v-if="table.rows.length === 0"
        class="mt-3 text-[13px]"
      >
        No files left in this scan.
      </p>
      <table
        v-else
        class="mt-3 w-full text-[13px]"
      >
        <thead class="text-left text-xs text-muted">
          <tr>
            <th class="py-1 font-normal">Owner</th>
            <th class="py-1 text-right font-normal">Space</th>
            <th class="py-1 text-right font-normal">Files</th>
            <th class="py-1 text-right font-normal">Share</th>
            <th class="py-1 pl-4 font-normal">Most of it in</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="row in table.rows"
            :key="row.index"
            class="border-t border-stroke align-top"
            :class="{ 'bg-accent/12': cleanupJob.ownerFilter?.index === row.index }"
          >
            <td class="py-1">
              <button
                type="button"
                class="flex items-center gap-1.5 text-left hover:underline"
                :aria-pressed="cleanupJob.ownerFilter?.index === row.index"
                :title="
                  cleanupJob.ownerFilter?.index === row.index
                    ? 'Show every owner’s files again'
                    : 'Show only this owner’s files in the folder view'
                "
                @click="filterBy(row)"
              >
                <FunnelX
                  v-if="cleanupJob.ownerFilter?.index === row.index"
                  class="size-3.5 shrink-0"
                />
                <Funnel
                  v-else
                  class="size-3.5 shrink-0 text-muted"
                />
                <span :class="{ 'text-muted': row.kind === 'unreadable' }">{{ row.name }}</span>
              </button>
            </td>
            <td class="py-1 text-right tabular-nums">{{ formatBytes(row.disk) }}</td>
            <td class="py-1 text-right tabular-nums">{{ row.files }}</td>
            <td class="py-1 text-right tabular-nums">
              {{ percentShare(row.disk, table.totalDisk) }}
            </td>
            <td class="py-1 pl-4 text-xs text-muted">
              <span
                v-for="folder in row.topFolders"
                :key="folder.id"
                class="block truncate"
                :title="folder.name"
              >
                {{ folder.name }} · {{ formatBytes(folder.disk) }}
              </span>
            </td>
          </tr>
        </tbody>
      </table>
      <p class="mt-2 text-xs text-muted">
        {{ formatBytes(table.totalDisk) }} in {{ plural(table.totalFiles, 'file') }}. Online-only
        cloud files are listed with their owner but take no space here.
      </p>
    </template>
  </section>
</template>
