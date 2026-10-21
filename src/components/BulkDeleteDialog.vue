<script setup lang="ts">
import { CircleCheck, LoaderCircle, ShieldAlert, TriangleAlert } from '@lucide/vue'
import { computed, ref } from 'vue'
import {
  appAdmin,
  canDelete,
  deleteSentence,
  outcomeLabel,
  previewDelete,
  removeMany,
  restartAsAdmin,
  summarize,
  type DeletePreview,
  type ItemResult,
  type PreviewStatus,
} from '@/lib/cleanup'
import { errorMessage } from '@/lib/compare'
import { formatBytes, plural } from '@/lib/format'

const emit = defineEmits<{ finished: [] }>()

type Phase = 'loading' | 'review' | 'running' | 'results'

const el = ref<HTMLDialogElement | null>(null)
const phase = ref<Phase>('loading')
const preview = ref<DeletePreview | null>(null)
const results = ref<ItemResult[]>([])
const error = ref('')

const STATUS_LABELS: Record<PreviewStatus, string> = {
  go: '',
  protected: 'Protected',
  reparse: 'Link, not followed',
  gone: 'Already gone',
  inside: 'Inside another picked item',
}

const going = computed(() => preview.value?.items.filter((item) => item.status === 'go') ?? [])
const left = computed(() => preview.value?.items.filter((item) => item.status !== 'go') ?? [])
const goIds = computed(() => going.value.map((item) => item.id))
const count = computed(() => preview.value?.goCount ?? 0)
const allowRecycle = computed(() => canDelete(count.value, false, appAdmin.elevated))
const allowPermanent = computed(() => canDelete(count.value, true, appAdmin.elevated))
const summary = computed(() => summarize(results.value))

/** Opens the dialog with a dry run of deleting `ids`; nothing is removed until a button is used. */
async function show(ids: number[]): Promise<void> {
  phase.value = 'loading'
  preview.value = null
  results.value = []
  error.value = ''
  el.value?.showModal()
  try {
    preview.value = await previewDelete(ids)
    phase.value = 'review'
  } catch (err) {
    error.value = errorMessage(err)
    phase.value = 'review'
  }
}

async function run(permanent: boolean): Promise<void> {
  if (phase.value !== 'review' || !canDelete(count.value, permanent, appAdmin.elevated)) {
    return
  }
  phase.value = 'running'
  error.value = ''
  try {
    results.value = await removeMany(goIds.value, permanent)
    phase.value = 'results'
    emit('finished')
  } catch (err) {
    error.value = errorMessage(err)
    phase.value = 'review'
  }
}

async function restart(): Promise<void> {
  try {
    await restartAsAdmin()
  } catch (err) {
    error.value = errorMessage(err)
  }
}

function close(): void {
  if (phase.value !== 'running') {
    el.value?.close()
  }
}

defineExpose({ show })
</script>

<template>
  <dialog
    ref="el"
    class="m-auto w-[min(38rem,92vw)] rounded-lg border border-stroke-strong bg-app p-5 text-fg shadow-xl backdrop:bg-fg/20"
    aria-labelledby="bulk-title"
    @cancel="phase === 'running' && $event.preventDefault()"
  >
    <h2
      id="bulk-title"
      class="font-display text-lg font-semibold"
    >
      <template v-if="phase === 'results'">Done</template>
      <template v-else-if="phase === 'running'">Deleting…</template>
      <template v-else>Delete {{ plural(count, 'item') }}?</template>
    </h2>

    <p
      v-if="phase === 'loading'"
      class="mt-4 flex items-center gap-2 text-[13px] text-muted"
    >
      <LoaderCircle class="size-4 animate-spin" />
      Checking what would be deleted…
    </p>

    <template v-else-if="phase === 'review' || phase === 'running'">
      <p
        v-if="preview"
        class="mt-1 text-[13px] text-muted tabular-nums"
      >
        {{ formatBytes(preview.goSize) }} in total. {{ deleteSentence(count, false) }}
      </p>
      <ul
        class="mt-3 max-h-64 overflow-y-auto rounded-md border border-stroke bg-card text-[13px]"
        aria-label="Items to delete"
      >
        <li
          v-for="item in going"
          :key="item.id"
          class="flex items-baseline gap-3 border-b border-stroke px-3 py-1.5 last:border-b-0"
        >
          <span
            class="min-w-0 flex-1 truncate"
            :title="item.path"
          >
            {{ item.path }}
          </span>
          <span class="shrink-0 text-xs text-muted tabular-nums">
            <template v-if="item.isFolder">{{ plural(item.files, 'file') }} · </template
            >{{ formatBytes(item.size) }}
          </span>
        </li>
        <li
          v-for="item in left"
          :key="item.id"
          class="flex items-baseline gap-3 border-b border-stroke px-3 py-1.5 text-muted last:border-b-0"
        >
          <span
            class="min-w-0 flex-1 truncate"
            :title="item.path"
          >
            {{ item.path }}
          </span>
          <span class="shrink-0 text-xs">
            Kept: {{ item.reason ?? STATUS_LABELS[item.status] }}
          </span>
        </li>
      </ul>

      <p
        v-if="!allowRecycle && count > 0"
        class="mt-3 flex items-start gap-2 text-[13px] text-warning"
      >
        <ShieldAlert class="mt-0.5 size-4 shrink-0" />
        Deleting several items needs administrator.
      </p>
      <p
        v-else-if="!allowPermanent && count > 0"
        class="mt-3 flex items-start gap-2 text-[13px] text-muted"
      >
        <ShieldAlert class="mt-0.5 size-4 shrink-0" />
        Deleting permanently needs administrator.
      </p>
    </template>

    <template v-else>
      <p
        class="mt-1 flex items-center gap-1.5 text-[13px]"
        :class="summary.failed > 0 ? 'text-warning' : 'text-success'"
        role="status"
      >
        <component
          :is="summary.failed > 0 ? TriangleAlert : CircleCheck"
          class="size-4"
        />
        {{ plural(summary.done, 'item') }} deleted, {{ formatBytes(summary.freed) }} freed<template
          v-if="summary.failed > 0"
          >; {{ summary.failed }} could not be deleted</template
        >.
      </p>
      <ul
        class="mt-3 max-h-64 overflow-y-auto rounded-md border border-stroke bg-card text-[13px]"
        aria-label="Results"
      >
        <li
          v-for="item in results"
          :key="item.id"
          class="flex items-baseline gap-3 border-b border-stroke px-3 py-1.5 last:border-b-0"
        >
          <span
            class="min-w-0 flex-1 truncate"
            :title="item.path"
          >
            {{ item.path }}
          </span>
          <span
            class="shrink-0 text-xs"
            :class="item.outcome === 'done' ? 'text-success' : 'text-danger'"
            :title="item.message ?? undefined"
          >
            {{ outcomeLabel(item.outcome) }}
          </span>
        </li>
      </ul>
    </template>

    <p
      v-if="error"
      class="mt-3 text-[13px] text-danger"
      role="alert"
    >
      {{ error }}
    </p>

    <div class="mt-5 flex flex-wrap justify-end gap-2">
      <template v-if="phase === 'results'">
        <button
          type="button"
          class="h-8 rounded-md bg-accent px-4 font-semibold text-on-accent hover:bg-accent-hover"
          @click="close"
        >
          Close
        </button>
      </template>
      <template v-else>
        <button
          v-if="!appAdmin.elevated && count > 0"
          type="button"
          class="mr-auto flex h-8 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover"
          title="Windows asks first"
          @click="restart"
        >
          <ShieldAlert class="size-4" />
          Restart as administrator
        </button>
        <button
          type="button"
          class="h-8 rounded-md border border-stroke bg-card px-4 hover:bg-card-hover disabled:opacity-60"
          :disabled="phase === 'running'"
          @click="close"
        >
          Cancel
        </button>
        <button
          type="button"
          class="flex h-8 items-center gap-1.5 rounded-md border border-danger/40 bg-card px-4 text-danger hover:bg-danger-bg disabled:opacity-50"
          :disabled="phase !== 'review' || !allowPermanent"
          :title="
            allowPermanent ? 'Skips the Recycle Bin; cannot be undone' : 'Needs administrator'
          "
          @click="run(true)"
        >
          Delete permanently
        </button>
        <button
          type="button"
          class="flex h-8 items-center gap-1.5 rounded-md bg-accent px-4 font-semibold text-on-accent hover:bg-accent-hover disabled:opacity-50"
          :disabled="phase !== 'review' || !allowRecycle"
          @click="run(false)"
        >
          <LoaderCircle
            v-if="phase === 'running'"
            class="size-4 animate-spin"
          />
          Move to Recycle Bin
        </button>
      </template>
    </div>
  </dialog>
</template>
