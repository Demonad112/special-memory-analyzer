<script setup lang="ts">
import { ChevronDown, ChevronRight, Hourglass, LoaderCircle } from '@lucide/vue'
import { computed, ref, watch } from 'vue'
import {
  cleanupJob,
  DEFAULT_STALE_YEARS,
  lastAccessNote,
  loadStale,
  loadStaleFiles,
  STALE_YEARS,
  type StaleFile,
  type StaleReport,
} from '@/lib/cleanup'
import { errorMessage } from '@/lib/errors'
import { formatBytes, formatDate, percentShare, plural } from '@/lib/format'

defineProps<{ readOnly?: boolean }>()
const emit = defineEmits<{ review: [ids: number[]] }>()

const years = ref(DEFAULT_STALE_YEARS)
const notOpened = ref(false)
const report = ref<StaleReport | null>(null)
const loading = ref(false)
const error = ref('')
/** The group whose files are shown, and those files. */
const open = ref<number | null>(null)
const files = ref<StaleFile[]>([])
/** Nothing is ticked until the user ticks it. */
const ticked = ref(new Set<number>())
/** Bumped by every load, so a late answer writes nothing. */
let seq = 0

const tickedSize = computed(() =>
  files.value.filter((file) => ticked.value.has(file.id)).reduce((sum, f) => sum + f.size, 0),
)

async function load(): Promise<void> {
  const mine = ++seq
  loading.value = true
  error.value = ''
  open.value = null
  files.value = []
  ticked.value = new Set()
  try {
    const next = await loadStale(years.value, notOpened.value)
    if (mine === seq) {
      report.value = next
    }
  } catch (err) {
    if (mine === seq) {
      report.value = null
      error.value = errorMessage(err)
    }
  } finally {
    if (mine === seq) {
      loading.value = false
    }
  }
}

// A new threshold, filter or scan gives a new report.
watch([years, notOpened, () => cleanupJob.finishedAt], () => void load(), { immediate: true })

async function toggleGroup(id: number): Promise<void> {
  if (open.value === id) {
    open.value = null
    files.value = []
    ticked.value = new Set()
    return
  }
  const mine = ++seq
  open.value = id
  files.value = []
  ticked.value = new Set()
  try {
    const next = await loadStaleFiles(id, years.value, notOpened.value)
    if (mine === seq) {
      files.value = next
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

defineExpose({ reload: load })
</script>

<template>
  <section
    class="rounded-lg border border-stroke bg-card p-4"
    aria-labelledby="stale-title"
  >
    <div class="flex flex-wrap items-center gap-2">
      <Hourglass class="size-4 text-accent" />
      <h2
        id="stale-title"
        class="font-display text-[15px] font-semibold"
      >
        Stale data
      </h2>
      <label class="ml-auto flex items-center gap-2 text-[13px]">
        Not modified in
        <select
          v-model.number="years"
          class="h-8 rounded-md border border-stroke bg-app px-2"
        >
          <option
            v-for="choice in STALE_YEARS"
            :key="choice"
            :value="choice"
          >
            {{ plural(choice, 'year') }}
          </option>
        </select>
      </label>
      <label class="flex items-center gap-2 text-[13px]">
        <input
          v-model="notOpened"
          type="checkbox"
        />
        Also not opened
      </label>
    </div>
    <p
      v-if="notOpened && report"
      class="mt-2 text-xs text-muted"
    >
      {{ lastAccessNote(report.lastAccess) }}
    </p>

    <p
      v-if="loading"
      class="mt-3 flex items-center gap-2 text-[13px] text-muted"
    >
      <LoaderCircle class="size-4 animate-spin" />
      Adding up old files…
    </p>
    <p
      v-else-if="error"
      class="mt-3 text-[13px] text-danger"
      role="alert"
    >
      {{ error }}
    </p>

    <template v-if="report && !loading">
      <p class="mt-3 text-[13px]">
        <template v-if="report.staleFiles === 0">
          Nothing older than {{ plural(report.years, 'year') }}.
        </template>
        <template v-else>
          {{ formatBytes(report.staleBytes) }} in {{ plural(report.staleFiles, 'file') }} ({{
            percentShare(report.staleBytes, report.totalBytes)
          }}
          of the scan) not modified in {{ plural(report.years, 'year')
          }}<template v-if="report.notOpened"> and not opened</template>.
        </template>
      </p>
      <p
        v-if="report.unknownFiles > 0 || report.cloudFiles > 0"
        class="text-xs text-muted"
      >
        Not counted:
        <template v-if="report.unknownFiles > 0">
          {{ plural(report.unknownFiles, 'file') }} ({{ formatBytes(report.unknownBytes) }}) with
          the date unknown (missing, in the future or before 1980);
        </template>
        <template v-if="report.cloudFiles > 0">
          {{ plural(report.cloudFiles, 'online-only file') }} (not on this disk).
        </template>
      </p>

      <table
        v-if="report.groups.length > 0"
        class="mt-3 w-full text-[13px]"
      >
        <thead class="text-left text-xs text-muted">
          <tr>
            <th class="py-1 font-normal">Folder</th>
            <th class="py-1 text-right font-normal">Stale</th>
            <th class="py-1 text-right font-normal">Files</th>
            <th class="py-1 text-right font-normal">Of the folder</th>
            <th class="py-1 text-right font-normal">Newest change</th>
          </tr>
        </thead>
        <tbody>
          <template
            v-for="group in report.groups"
            :key="group.id"
          >
            <tr class="border-t border-stroke">
              <td class="py-1">
                <button
                  type="button"
                  class="flex items-center gap-1 text-left hover:underline"
                  :aria-expanded="open === group.id"
                  @click="toggleGroup(group.id)"
                >
                  <ChevronDown
                    v-if="open === group.id"
                    class="size-3.5 shrink-0"
                  />
                  <ChevronRight
                    v-else
                    class="size-3.5 shrink-0"
                  />
                  {{ group.name }}
                </button>
              </td>
              <td class="py-1 text-right tabular-nums">{{ formatBytes(group.staleBytes) }}</td>
              <td class="py-1 text-right tabular-nums">{{ group.staleFiles }}</td>
              <td class="py-1 text-right tabular-nums">
                {{ percentShare(group.staleBytes, group.folderBytes) }}
              </td>
              <td class="py-1 text-right text-xs text-muted tabular-nums">
                {{ group.newestMs === null ? '' : formatDate(group.newestMs) }}
              </td>
            </tr>
            <tr v-if="open === group.id">
              <td
                colspan="5"
                class="pb-2 pl-5"
              >
                <p class="text-xs text-muted">
                  The largest stale files. Tick the ones to remove; the folder itself is never
                  deleted from here, because it can hold newer files too.
                </p>
                <ul>
                  <li
                    v-for="file in files"
                    :key="file.id"
                  >
                    <label class="flex items-baseline gap-2 py-0.5">
                      <input
                        type="checkbox"
                        :checked="ticked.has(file.id)"
                        @change="toggle(file.id)"
                      />
                      <span
                        class="min-w-0 flex-1 truncate"
                        :title="file.path"
                      >
                        {{ file.path }}
                      </span>
                      <span class="w-24 shrink-0 text-right text-xs text-muted tabular-nums">
                        {{ file.modifiedMs === null ? '' : formatDate(file.modifiedMs) }}
                      </span>
                      <span class="w-20 shrink-0 text-right text-xs tabular-nums">
                        {{ formatBytes(file.size) }}
                      </span>
                    </label>
                  </li>
                </ul>
                <button
                  v-if="!readOnly"
                  type="button"
                  class="mt-2 h-8 rounded-md bg-accent px-3 text-[13px] font-semibold text-on-accent hover:bg-accent-hover disabled:opacity-50"
                  :disabled="ticked.size === 0"
                  @click="emit('review', [...ticked])"
                >
                  Review {{ plural(ticked.size, 'file')
                  }}<template v-if="ticked.size > 0"> · {{ formatBytes(tickedSize) }}</template>
                </button>
              </td>
            </tr>
          </template>
        </tbody>
      </table>
    </template>
  </section>
</template>
