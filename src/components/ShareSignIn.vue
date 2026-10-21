<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { errorMessage } from '@/lib/compare'
import { connectShare, shareOf } from '@/lib/record'

const props = defineProps<{ paths: string[] }>()

const shares = computed(() => [
  ...new Set(props.paths.map(shareOf).filter((share): share is string => share !== null)),
])
const share = ref('')
const username = ref('')
const password = ref('')
const busy = ref(false)
const message = ref('')
const failed = ref(false)

watch(
  shares,
  (list) => {
    if (!list.includes(share.value)) {
      share.value = list[0] ?? ''
    }
  },
  { immediate: true },
)

async function connect() {
  busy.value = true
  message.value = ''
  failed.value = false
  try {
    const connected = await connectShare(share.value, username.value.trim(), password.value)
    message.value = `Signed in to ${connected}. It stays signed in until you sign out of Windows.`
  } catch (error) {
    failed.value = true
    message.value = errorMessage(error)
  } finally {
    // The password is not kept once Windows has it.
    password.value = ''
    busy.value = false
  }
}
</script>

<template>
  <details
    v-if="shares.length > 0"
    class="mt-3 text-[13px]"
  >
    <summary class="w-fit cursor-pointer text-muted hover:text-fg">
      Network share asks for a sign-in?
    </summary>
    <form
      class="mt-2 flex flex-wrap items-end gap-2"
      @submit.prevent="connect"
    >
      <label class="flex flex-col gap-1">
        Share
        <select
          v-model="share"
          class="h-8 rounded-md border border-stroke border-b-stroke-strong bg-card px-2 text-[13px] outline-none focus:border-b-2 focus:border-b-accent"
        >
          <option
            v-for="item in shares"
            :key="item"
            :value="item"
          >
            {{ item }}
          </option>
        </select>
      </label>
      <label class="flex flex-col gap-1">
        User name
        <input
          v-model="username"
          type="text"
          autocomplete="off"
          placeholder="DOMAIN\name"
          class="h-8 w-44 rounded-md border border-stroke border-b-stroke-strong bg-card px-2 text-[13px] outline-none focus:border-b-2 focus:border-b-accent"
        />
      </label>
      <label class="flex flex-col gap-1">
        Password
        <input
          v-model="password"
          type="password"
          autocomplete="off"
          class="h-8 w-44 rounded-md border border-stroke border-b-stroke-strong bg-card px-2 text-[13px] outline-none focus:border-b-2 focus:border-b-accent"
        />
      </label>
      <button
        type="submit"
        class="h-8 rounded-md bg-accent px-4 font-semibold text-on-accent hover:bg-accent-hover disabled:opacity-50"
        :disabled="busy || share === '' || username.trim() === ''"
      >
        {{ busy ? 'Signing in…' : 'Sign in' }}
      </button>
    </form>
    <p
      v-if="message"
      class="mt-2"
      :class="failed ? 'text-danger' : 'text-muted'"
      role="status"
    >
      {{ message }}
    </p>
    <p class="mt-1 text-muted">The password goes to Windows once and is not saved by this app.</p>
  </details>
</template>
