<script setup lang="ts">
import { CircleCheck, LoaderCircle, ShieldAlert } from '@lucide/vue'
import { isTauri } from '@tauri-apps/api/core'
import { computed, onMounted, ref } from 'vue'
import {
  canClean,
  loadQuickWins,
  restartAsAdmin,
  runQuickClean,
  type QuickId,
  type QuickList,
  type QuickResult,
} from '@/lib/cleanup'
import { errorMessage } from '@/lib/compare'
import { formatBytes, plural } from '@/lib/format'

const emit = defineEmits<{ cleaned: [] }>()

const list = ref<QuickList | null>(null)
const loading = ref(false)
const cleaning = ref(false)
const chosen = ref(new Set<QuickId>())
/** Not ticked by default: they hold things the user put there. */
const personal = new Set<QuickId>(['recycleBin', 'oldInstallers'])
const result = ref<QuickResult | null>(null)
const error = ref('')

const chosenItems = computed(
  () => list.value?.items.filter((item) => chosen.value.has(item.id)) ?? [],
)
const chosenSize = computed(() => chosenItems.value.reduce((sum, item) => sum + item.size, 0))
const freed = computed(
  () => result.value?.report.items.reduce((sum, item) => sum + item.freed, 0) ?? 0,
)
const needsRestart = computed(
  () =>
    list.value !== null &&
    !list.value.elevated &&
    list.value.items.some((item) => item.needsAdmin && item.size > 0),
)

async function refresh(): Promise<void> {
  if (!isTauri()) {
    return
  }
  loading.value = true
  error.value = ''
  try {
    const loaded = await loadQuickWins()
    list.value = loaded
    // Tick the throw-away caches; the Recycle Bin and Downloads are the user's to choose.
    chosen.value = new Set(
      loaded.items
        .filter((item) => canClean(item, loaded.elevated) && !personal.has(item.id))
        .map((item) => item.id),
    )
  } catch (err) {
    error.value = errorMessage(err)
  } finally {
    loading.value = false
  }
}

function toggle(id: QuickId): void {
  const next = new Set(chosen.value)
  if (!next.delete(id)) {
    next.add(id)
  }
  chosen.value = next
}

const confirmEl = ref<HTMLDialogElement | null>(null)

async function clean(): Promise<void> {
  confirmEl.value?.close()
  cleaning.value = true
  error.value = ''
  try {
    result.value = await runQuickClean([...chosen.value])
    emit('cleaned')
  } catch (err) {
    error.value = errorMessage(err)
  } finally {
    cleaning.value = false
  }
  await refresh()
}

async function restart(): Promise<void> {
  try {
    await restartAsAdmin()
  } catch (err) {
    error.value = errorMessage(err)
  }
}

onMounted(refresh)
</script>

<template>
  <section aria-labelledby="quick-title">
    <div class="mb-2 flex items-center justify-between gap-3">
      <h2
        id="quick-title"
        class="text-[13px] font-semibold text-muted"
      >
        Quick cleanup
      </h2>
      <LoaderCircle
        v-if="loading"
        class="size-4 animate-spin text-muted"
      />
    </div>

    <div
      v-if="list"
      class="grid grid-cols-[repeat(auto-fill,minmax(16rem,1fr))] gap-3"
    >
      <label
        v-for="item in list.items"
        :key="item.id"
        class="flex min-w-0 items-start gap-3 rounded-lg border bg-card px-4 py-3 transition-colors"
        :class="[
          chosen.has(item.id) ? 'border-accent/60' : 'border-stroke',
          canClean(item, list.elevated) ? 'hover:bg-card-hover' : 'opacity-70',
        ]"
      >
        <input
          type="checkbox"
          class="mt-1 size-4 shrink-0 accent-accent"
          :checked="chosen.has(item.id)"
          :disabled="!canClean(item, list.elevated) || cleaning"
          @change="toggle(item.id)"
        />
        <span class="min-w-0 flex-1">
          <span class="flex items-baseline justify-between gap-2">
            <span class="truncate font-semibold">{{ item.title }}</span>
            <span class="shrink-0 tabular-nums">{{ formatBytes(item.size) }}</span>
          </span>
          <span class="mt-0.5 block text-xs leading-4 text-muted">{{ item.description }}</span>
          <span
            v-if="item.needsAdmin && !list.elevated"
            class="mt-1 flex items-center gap-1 text-xs text-warning"
          >
            <ShieldAlert class="size-3.5" />
            Needs administrator
          </span>
        </span>
      </label>
    </div>

    <div
      v-if="list"
      class="mt-3 flex flex-wrap items-center gap-3"
    >
      <button
        type="button"
        class="flex h-8 items-center gap-1.5 rounded-md bg-accent px-4 font-semibold text-on-accent hover:bg-accent-hover disabled:opacity-50"
        :disabled="chosen.size === 0 || cleaning"
        @click="confirmEl?.showModal()"
      >
        <LoaderCircle
          v-if="cleaning"
          class="size-4 animate-spin"
        />
        {{ cleaning ? 'Cleaning…' : `Clean ${formatBytes(chosenSize)}` }}
      </button>
      <button
        v-if="needsRestart"
        type="button"
        class="flex h-8 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover"
        title="Windows asks first; the system cleanups then become available"
        @click="restart"
      >
        <ShieldAlert class="size-4" />
        Restart as administrator
      </button>
      <p
        v-if="result && !cleaning"
        class="flex items-center gap-1.5 text-[13px] text-success"
        role="status"
      >
        <CircleCheck class="size-4" />
        Freed {{ formatBytes(freed) }}.
        <template v-if="result.reportId">Saved to Reports.</template>
      </p>
    </div>
    <p
      v-if="error"
      class="mt-2 text-[13px] text-danger"
      role="alert"
    >
      {{ error }}
    </p>

    <dialog
      ref="confirmEl"
      class="m-auto w-[min(30rem,90vw)] rounded-lg border border-stroke-strong bg-app p-5 text-fg shadow-xl backdrop:bg-fg/20"
    >
      <h2 class="font-display text-lg font-semibold">Clean {{ formatBytes(chosenSize) }}?</h2>
      <ul class="mt-3 space-y-1 text-[13px]">
        <li
          v-for="item in chosenItems"
          :key="item.id"
          class="flex justify-between gap-4"
        >
          <span>{{ item.title }}</span>
          <span class="text-muted tabular-nums">
            {{ formatBytes(item.size) }} · {{ plural(item.files, 'file') }}
          </span>
        </li>
      </ul>
      <p class="mt-3 text-[13px] text-muted">
        Files in use are skipped. Temp files and caches are deleted for good<template
          v-if="chosen.has('recycleBin')"
          >, and so is everything in the Recycle Bin</template
        >; old setup files go to the Recycle Bin.
      </p>
      <div class="mt-5 flex justify-end gap-2">
        <button
          type="button"
          class="h-8 rounded-md border border-stroke bg-card px-4 hover:bg-card-hover"
          @click="confirmEl?.close()"
        >
          Cancel
        </button>
        <button
          type="button"
          class="h-8 rounded-md bg-accent px-4 font-semibold text-on-accent hover:bg-accent-hover"
          @click="clean"
        >
          Clean
        </button>
      </div>
    </dialog>
  </section>
</template>
