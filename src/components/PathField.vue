<script setup lang="ts">
import { FolderOpen, Server } from '@lucide/vue'
import { computed, ref } from 'vue'
import ServerShareList from '@/components/ServerShareList.vue'
import { profiles } from '@/lib/servers'

defineProps<{
  label: string
  hint: string
  disabled?: boolean
  /** A folder is being dragged over this field. */
  dropTarget?: boolean
}>()
const path = defineModel<string>({ required: true })
const emit = defineEmits<{ browse: []; submit: [] }>()

// Pick a share of a saved server (Settings > Servers) instead of typing its path.
const showServers = ref(false)
const profileId = ref('')
const profile = computed(
  () => profiles.value.find((item) => item.id === profileId.value) ?? profiles.value[0] ?? null,
)

function use(picked: string): void {
  path.value = picked
  showServers.value = false
}
</script>

<template>
  <div class="min-w-0">
    <label
      class="block min-w-0 rounded-lg border bg-card px-4 py-3 transition-colors"
      :class="dropTarget ? 'border-accent bg-accent/8' : 'border-stroke'"
    >
      <span class="flex items-baseline gap-2">
        <span class="font-semibold">{{ label }}</span>
        <span class="truncate text-xs text-muted">{{ hint }}</span>
      </span>
      <span class="mt-2 flex gap-2">
        <input
          v-model="path"
          type="text"
          spellcheck="false"
          autocomplete="off"
          :disabled="disabled"
          :placeholder="dropTarget ? 'Drop the folder here' : 'Type, paste or drop a folder path'"
          class="h-8 min-w-0 flex-1 rounded-md border border-stroke border-b-stroke-strong bg-card px-2.5 text-[13px] outline-none placeholder:text-faint focus:border-b-2 focus:border-b-accent disabled:opacity-60"
          @keydown.enter="emit('submit')"
        />
        <button
          type="button"
          class="flex h-8 shrink-0 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover disabled:opacity-60"
          :disabled="disabled"
          @click.prevent="emit('browse')"
        >
          <FolderOpen
            class="size-4"
            :stroke-width="1.75"
          />
          Browse
        </button>
        <button
          v-if="profiles.length > 0"
          type="button"
          class="flex h-8 shrink-0 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 text-[13px] hover:bg-card-hover disabled:opacity-60"
          :disabled="disabled"
          :aria-expanded="showServers"
          @click.prevent="showServers = !showServers"
        >
          <Server
            class="size-4"
            :stroke-width="1.75"
          />
          Servers
        </button>
      </span>
    </label>
    <div
      v-if="showServers && profile"
      class="mt-1 rounded-lg border border-stroke bg-card px-4 py-3 text-[13px]"
    >
      <label class="mb-2 flex items-center gap-2">
        Server
        <select
          :value="profile.id"
          class="h-8 rounded-md border border-stroke border-b-stroke-strong bg-card px-2 text-[13px] outline-none focus:border-b-2 focus:border-b-accent"
          @change="profileId = ($event.target as HTMLSelectElement).value"
        >
          <option
            v-for="item in profiles"
            :key="item.id"
            :value="item.id"
          >
            {{ item.name }} ({{ item.server }})
          </option>
        </select>
      </label>
      <ServerShareList
        :key="profile.id"
        :profile="profile"
        pick-label="Use"
        @pick="use"
      />
    </div>
  </div>
</template>
