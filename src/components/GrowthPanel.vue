<script setup lang="ts">
import { LoaderCircle, TrendingUp } from '@lucide/vue'
import { ref, watch } from 'vue'
import {
  cleanupJob,
  forecastLine,
  growthLabel,
  loadGrowth,
  loadHistory,
  type Growth,
  type History,
} from '@/lib/cleanup'
import { errorMessage } from '@/lib/errors'
import { formatBytes, formatDate, formatDelta, plural } from '@/lib/format'

const history = ref<History | null>(null)
const growth = ref<Growth | null>(null)
/** The earlier snapshot to compare with; empty means the previous scan. */
const from = ref('')
const loading = ref(false)
const error = ref('')
/** Bumped by every load and new scan, so a late answer writes nothing. */
let seq = 0

/** Snapshots a comparison can start from: every one except the scan on screen. */
function earlier(info: History | null): History['snapshots'] {
  return info ? info.snapshots.filter((snapshot) => !snapshot.current) : []
}

async function load(): Promise<void> {
  const mine = ++seq
  loading.value = true
  error.value = ''
  try {
    const info = await loadHistory()
    if (mine !== seq) {
      return
    }
    history.value = info
    growth.value = null
    if (earlier(info).length > 0) {
      const next = await loadGrowth(from.value ? Number(from.value) : undefined)
      if (mine === seq) {
        growth.value = next
      }
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

// A new scan has a new history; the choice of earlier scan starts over.
watch(
  () => cleanupJob.finishedAt,
  () => {
    from.value = ''
    void load()
  },
  { immediate: true },
)

defineExpose({ reload: load })
</script>

<template>
  <section
    class="rounded-lg border border-stroke bg-card p-4"
    aria-labelledby="growth-title"
  >
    <div class="flex flex-wrap items-center gap-2">
      <TrendingUp class="size-4 text-accent" />
      <h2
        id="growth-title"
        class="font-display text-[15px] font-semibold"
      >
        What grew
      </h2>
      <label
        v-if="earlier(history).length > 1"
        class="ml-auto flex items-center gap-2 text-[13px]"
      >
        Compare with
        <select
          v-model="from"
          class="h-8 rounded-md border border-stroke bg-app px-2"
          @change="load"
        >
          <option value="">The previous scan</option>
          <option
            v-for="snapshot in earlier(history)"
            :key="snapshot.takenMs"
            :value="String(snapshot.takenMs)"
          >
            {{ formatDate(snapshot.takenMs) }} · {{ formatBytes(snapshot.size) }}
          </option>
        </select>
      </label>
    </div>

    <p
      v-if="loading"
      class="mt-3 flex items-center gap-2 text-[13px] text-muted"
    >
      <LoaderCircle class="size-4 animate-spin" />
      Comparing with earlier scans…
    </p>
    <p
      v-else-if="error"
      class="mt-3 text-[13px] text-danger"
      role="alert"
    >
      {{ error }}
    </p>

    <template v-if="history && !loading">
      <p
        v-if="forecastLine(history.forecast)"
        class="mt-3 text-[13px]"
      >
        {{ forecastLine(history.forecast) }}
      </p>
      <p
        v-if="earlier(history).length === 0"
        class="mt-2 text-xs text-muted"
      >
        No earlier scan of this folder is saved yet. Scan it again later to see what grew. Scan
        history can be turned off in Settings.
      </p>
      <template v-else-if="growth">
        <p class="mt-3 text-[13px]">
          Since {{ formatDate(growth.fromMs) }}: {{ formatBytes(growth.before) }} →
          {{ formatBytes(growth.after) }} ({{ formatDelta(growth.delta) }}).
        </p>
        <p
          v-if="growth.rows.length === 0"
          class="text-xs text-muted"
        >
          No folder changed size.
        </p>
        <table
          v-else
          class="mt-2 w-full text-[13px]"
        >
          <thead class="text-left text-xs text-muted">
            <tr>
              <th class="py-1 font-normal">Folder</th>
              <th class="py-1 font-normal">Change</th>
              <th class="py-1 text-right font-normal">Before</th>
              <th class="py-1 text-right font-normal">Now</th>
              <th class="py-1 text-right font-normal">Growth</th>
              <th class="py-1 text-right font-normal">%</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="row in growth.rows"
              :key="row.path"
              class="border-t border-stroke"
            >
              <td
                class="max-w-0 truncate py-1"
                :title="row.path"
              >
                {{ row.path }}
              </td>
              <td class="py-1 text-xs text-muted">{{ growthLabel(row.change) }}</td>
              <td class="py-1 text-right tabular-nums">{{ formatBytes(row.before) }}</td>
              <td class="py-1 text-right tabular-nums">{{ formatBytes(row.after) }}</td>
              <td
                class="py-1 text-right tabular-nums"
                :class="row.delta > 0 ? 'text-danger' : 'text-muted'"
              >
                {{ formatDelta(row.delta) }}
              </td>
              <td class="py-1 text-right text-xs text-muted tabular-nums">
                {{ row.percent === null ? '' : `${row.percent.toFixed(0)}%` }}
              </td>
            </tr>
          </tbody>
        </table>
        <p
          v-if="growth.hidden > 0"
          class="mt-1 text-xs text-muted"
        >
          {{ plural(growth.hidden, 'smaller change') }} not listed.
        </p>
        <p class="mt-2 text-xs text-muted">
          Folders are compared to three levels deep, and only those holding at least 0.5% of the
          scan are saved, so a folder can show as new or gone when it crossed that line.
        </p>
      </template>
    </template>
  </section>
</template>
