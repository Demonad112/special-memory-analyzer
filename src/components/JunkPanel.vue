<script setup lang="ts">
import { LoaderCircle, Sparkles } from '@lucide/vue'
import { computed, onMounted, ref } from 'vue'
import { loadJunk, type JunkGroup } from '@/lib/cleanup'
import { errorMessage } from '@/lib/compare'
import { formatBytes, plural } from '@/lib/format'

const emit = defineEmits<{ review: [ids: number[]] }>()

const groups = ref<JunkGroup[]>([])
const loading = ref(true)
const error = ref('')
/** Nothing is ticked until the user ticks it. */
const ticked = ref(new Set<number>())

/** The nodes behind the ticked rows. */
const tickedIds = computed(() =>
  groups.value
    .flatMap((group) => group.items)
    .filter((item) => ticked.value.has(item.id))
    .flatMap((item) => item.ids),
)

const sizeTicked = computed(() =>
  groups.value
    .flatMap((group) => group.items)
    .filter((item) => ticked.value.has(item.id))
    .reduce((sum, item) => sum + item.size, 0),
)

onMounted(async () => {
  try {
    groups.value = (await loadJunk()).filter((group) => group.items.length > 0)
  } catch (err) {
    error.value = errorMessage(err)
  } finally {
    loading.value = false
  }
})

function toggle(id: number): void {
  const next = new Set(ticked.value)
  if (!next.delete(id)) {
    next.add(id)
  }
  ticked.value = next
}

function groupTicked(group: JunkGroup): boolean {
  return group.items.every((item) => ticked.value.has(item.id))
}

function toggleGroup(group: JunkGroup): void {
  const next = new Set(ticked.value)
  const all = groupTicked(group)
  for (const item of group.items) {
    if (all) {
      next.delete(item.id)
    } else {
      next.add(item.id)
    }
  }
  ticked.value = next
}

/** Drops ids that were deleted so they can't be ticked again. */
function forget(ids: number[]): void {
  const gone = new Set(ids)
  groups.value = groups.value
    .map((group) => ({
      ...group,
      // A reported row has no ids and is never deleted, so it stays.
      items: group.items.filter(
        (item) => item.ids.length === 0 || !item.ids.every((id) => gone.has(id)),
      ),
    }))
    .filter((group) => group.items.length > 0)
  const left = new Set(groups.value.flatMap((group) => group.items.map((item) => item.id)))
  ticked.value = new Set([...ticked.value].filter((id) => left.has(id)))
}

/** Matches not on a row: a collapsed row stands for several, a reported row for one. */
function unlisted(group: JunkGroup): number {
  return group.count - group.items.reduce((sum, item) => sum + Math.max(item.ids.length, 1), 0)
}

async function reload(): Promise<void> {
  loading.value = true
  ticked.value = new Set()
  try {
    groups.value = (await loadJunk()).filter((group) => group.items.length > 0)
  } catch (err) {
    error.value = errorMessage(err)
  } finally {
    loading.value = false
  }
}

defineExpose({ forget, reload })
</script>

<template>
  <section
    class="rounded-lg border border-stroke bg-card p-4"
    aria-labelledby="junk-title"
  >
    <div class="flex items-center gap-2">
      <Sparkles class="size-4 text-accent" />
      <h2
        id="junk-title"
        class="font-display text-[15px] font-semibold"
      >
        Suggested cleanups
      </h2>
      <span class="text-xs text-muted">Nothing is ticked for you.</span>
      <button
        type="button"
        class="ml-auto h-8 rounded-md bg-accent px-3 text-[13px] font-semibold text-on-accent hover:bg-accent-hover disabled:opacity-50"
        :disabled="ticked.size === 0"
        @click="emit('review', tickedIds)"
      >
        Review {{ plural(ticked.size, 'item')
        }}<template v-if="ticked.size > 0"> · {{ formatBytes(sizeTicked) }}</template>
      </button>
    </div>

    <p
      v-if="loading"
      class="mt-3 flex items-center gap-2 text-[13px] text-muted"
    >
      <LoaderCircle class="size-4 animate-spin" />
      Looking for junk…
    </p>
    <p
      v-else-if="error"
      class="mt-3 text-[13px] text-danger"
      role="alert"
    >
      {{ error }}
    </p>
    <p
      v-else-if="groups.length === 0"
      class="mt-3 text-[13px] text-muted"
    >
      Nothing obvious to clean in this scan.
    </p>

    <div
      v-for="group in groups"
      :key="`${group.kind}-${group.tier}`"
      class="mt-3"
    >
      <label class="flex items-baseline gap-2 text-[13px] font-semibold">
        <input
          v-if="group.tier !== 'never'"
          type="checkbox"
          :checked="groupTicked(group)"
          :aria-label="`Tick all ${group.title}`"
          @change="toggleGroup(group)"
        />
        {{ group.title }}
        <span class="font-normal text-muted tabular-nums">
          {{ plural(group.count, 'item') }} · {{ formatBytes(group.size) }}
        </span>
      </label>
      <p class="ml-6 text-xs text-muted">{{ group.description }}</p>
      <ul class="ml-6 mt-1 text-[13px]">
        <li
          v-for="item in group.items"
          :key="item.id"
        >
          <label class="flex items-baseline gap-2 py-0.5">
            <input
              v-if="item.ids.length > 0"
              type="checkbox"
              :checked="ticked.has(item.id)"
              @change="toggle(item.id)"
            />
            <span
              class="min-w-0 flex-1 truncate"
              :title="item.path"
            >
              {{ item.path }}
              <span
                v-if="item.ids.length > 1"
                class="text-muted"
              >
                · {{ plural(item.ids.length, 'folder') }}
              </span>
            </span>
            <span class="shrink-0 text-xs text-muted tabular-nums">
              {{ formatBytes(item.size) }}
            </span>
          </label>
        </li>
      </ul>
      <p
        v-if="unlisted(group) > 0"
        class="ml-6 text-xs text-muted"
      >
        and {{ unlisted(group) }} more not listed.
      </p>
    </div>
  </section>
</template>
