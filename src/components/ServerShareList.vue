<script setup lang="ts">
import { LoaderCircle } from '@lucide/vue'
import { onBeforeUnmount, ref } from 'vue'
import ShareSignIn from '@/components/ShareSignIn.vue'
import { errorMessage } from '@/lib/errors'
import {
  createShareLister,
  sharePath,
  signInShare,
  type ServerProfile,
  type ShareEntry,
} from '@/lib/servers'

const props = defineProps<{
  profile: ServerProfile
  /** What the button on each share says, such as "Use" or "Copy path". */
  pickLabel: string
}>()
const emit = defineEmits<{ pick: [path: string] }>()

const shares = ref<ShareEntry[] | null>(null)
const loading = ref(false)
const error = ref('')
const accessDenied = ref(false)

// Only the newest answer fills the list; the profile can change while a slow server thinks.
const lister = createShareLister()
let alive = true
onBeforeUnmount(() => {
  alive = false
})

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  accessDenied.value = false
  try {
    const answer = await lister(props.profile.server)
    if (!alive || answer === null) {
      return
    }
    shares.value = answer.error === null ? answer.shares : null
    error.value = answer.error ?? ''
    accessDenied.value = answer.accessDenied
    loading.value = false
  } catch (err) {
    if (alive) {
      shares.value = null
      error.value = errorMessage(err)
      loading.value = false
    }
  }
}
</script>

<template>
  <div class="text-[13px]">
    <button
      type="button"
      class="flex h-8 items-center gap-1.5 rounded-md border border-stroke bg-card px-3 hover:bg-card-hover disabled:opacity-60"
      :disabled="loading"
      @click="load"
    >
      <LoaderCircle
        v-if="loading"
        class="size-4 animate-spin"
      />
      List shares
    </button>
    <p
      v-if="error"
      class="mt-2 text-danger"
      role="alert"
    >
      {{ error }}
    </p>
    <ShareSignIn
      v-if="accessDenied"
      :paths="[signInShare(profile.server)]"
      :username="profile.user"
      open
      @signed-in="load"
    />
    <p
      v-if="shares !== null && shares.length === 0"
      class="mt-2 text-muted"
    >
      No disk shares were listed for {{ profile.server }}.
    </p>
    <ul
      v-if="shares !== null && shares.length > 0"
      class="mt-2 divide-y divide-stroke rounded-md border border-stroke"
      :aria-label="`Shares of ${profile.server}`"
    >
      <li
        v-for="share in shares"
        :key="share.name"
        class="flex items-center gap-2 px-3 py-1.5"
      >
        <span class="font-mono">{{ share.name }}</span>
        <span
          v-if="share.hidden"
          class="rounded border border-stroke px-1.5 text-xs text-muted"
          title="An administrative or hidden share"
        >
          hidden
        </span>
        <span class="min-w-0 flex-1 truncate text-xs text-muted">{{ share.comment }}</span>
        <button
          type="button"
          class="h-7 shrink-0 rounded-md border border-stroke bg-card px-2.5 hover:bg-card-hover"
          @click="emit('pick', sharePath(profile.server, share.name))"
        >
          {{ pickLabel }}
        </button>
      </li>
    </ul>
  </div>
</template>
