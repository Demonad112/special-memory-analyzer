<script setup lang="ts">
import { LoaderCircle, Undo2 } from '@lucide/vue'
import { onMounted, ref } from 'vue'
import {
  loadLastRun,
  restoreLast,
  restoreSentence,
  type LastRun,
  type RestoreResult,
} from '@/lib/cleanup'
import { errorMessage } from '@/lib/errors'
import { plural } from '@/lib/format'

const last = ref<LastRun | null>(null)
const asking = ref(false)
const busy = ref(false)
const results = ref<RestoreResult[]>([])
const error = ref('')

/** Reads the deletion log again; call it after a delete. */
async function reload(): Promise<void> {
  results.value = []
  asking.value = false
  try {
    last.value = await loadLastRun()
  } catch {
    last.value = null
  }
}

async function restore(): Promise<void> {
  busy.value = true
  error.value = ''
  try {
    results.value = await restoreLast()
    last.value = null
  } catch (err) {
    error.value = errorMessage(err)
  } finally {
    busy.value = false
    asking.value = false
  }
}

onMounted(() => void reload())
defineExpose({ reload })
</script>

<template>
  <div
    v-if="last || results.length > 0 || error"
    class="mb-4 flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px] text-muted"
    role="status"
  >
    <template v-if="results.length > 0">
      <span>{{ restoreSentence(results) }} Scan again to refresh the sizes.</span>
      <ul class="w-full text-xs">
        <li
          v-for="item in results.filter((result) => result.outcome !== 'restored')"
          :key="item.path"
          class="truncate"
          :title="item.path"
        >
          {{ item.path }}: {{ item.message }}
        </li>
      </ul>
    </template>
    <template v-else-if="last">
      <span>
        Last cleanup: {{ plural(last.items, 'item') }} on {{ last.time }}
        <template v-if="last.recycled < last.items">
          ({{ last.recycled }} in the Recycle Bin)
        </template>
      </span>
      <button
        v-if="!asking"
        type="button"
        class="flex h-7 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 hover:bg-card-hover disabled:opacity-60"
        :disabled="busy || last.recycled === 0"
        @click="asking = true"
      >
        <Undo2 class="size-4" />
        Restore last cleanup
      </button>
      <template v-else>
        <span class="text-fg">Put back {{ plural(last.recycled, 'item') }}?</span>
        <button
          type="button"
          class="flex h-7 items-center gap-1.5 rounded-md bg-accent px-3 font-semibold text-on-accent hover:bg-accent-hover disabled:opacity-60"
          :disabled="busy"
          @click="restore"
        >
          <LoaderCircle
            v-if="busy"
            class="size-4 animate-spin"
          />
          Yes, restore
        </button>
        <button
          type="button"
          class="h-7 rounded-md border border-stroke bg-card px-3 hover:bg-card-hover"
          :disabled="busy"
          @click="asking = false"
        >
          No
        </button>
      </template>
    </template>
    <span
      v-if="error"
      class="text-danger"
      role="alert"
    >
      {{ error }}
    </span>
  </div>
</template>
