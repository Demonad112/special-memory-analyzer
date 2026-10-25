<script setup lang="ts">
import { Server } from '@lucide/vue'
import { ref } from 'vue'
import ServerShareList from '@/components/ServerShareList.vue'
import { copyText } from '@/lib/clipboard'
import { errorMessage } from '@/lib/errors'
import {
  addProfile,
  profiles,
  removeProfile,
  updateProfile,
  type ProfileInput,
} from '@/lib/servers'

const empty = (): ProfileInput => ({ name: '', server: '', user: '', notes: '' })
const form = ref<ProfileInput>(empty())
/** The profile being edited, or null when the form adds a new one. */
const editing = ref<string | null>(null)
const formError = ref('')
const notice = ref('')

function save(): void {
  formError.value = ''
  const saved =
    editing.value === null
      ? addProfile(form.value) !== null
      : updateProfile(editing.value, form.value)
  if (!saved) {
    formError.value = 'Type the server name, for example fs01.'
    return
  }
  cancel()
}

function edit(id: string): void {
  const profile = profiles.value.find((item) => item.id === id)
  if (profile) {
    editing.value = id
    form.value = {
      name: profile.name,
      server: profile.server,
      user: profile.user,
      notes: profile.notes,
    }
  }
}

function cancel(): void {
  editing.value = null
  form.value = empty()
  formError.value = ''
}

async function copy(path: string): Promise<void> {
  try {
    await copyText(path)
    notice.value = `Copied ${path}`
  } catch (err) {
    notice.value = errorMessage(err)
  }
}

const field =
  'h-8 w-full rounded-md border border-stroke border-b-stroke-strong bg-card px-2 text-[13px] outline-none focus:border-b-2 focus:border-b-accent'
</script>

<template>
  <section class="mt-3 rounded-lg border border-stroke bg-card px-5 py-4">
    <div class="flex items-center gap-4">
      <Server
        class="size-5 shrink-0"
        :stroke-width="1.5"
      />
      <span class="flex-1">
        <span class="block">Servers</span>
        <span class="block text-xs text-muted">
          Saved servers for Compare, Record, Readiness and Disk Cleanup. Access uses your Windows
          sign-in. If a server refuses it, the sign-in form connects for this session only.
          Passwords are never saved.
        </span>
      </span>
    </div>

    <ul
      v-if="profiles.length > 0"
      class="mt-3 text-[13px]"
    >
      <li
        v-for="profile in profiles"
        :key="profile.id"
        class="border-t border-stroke py-2"
      >
        <div class="flex items-center gap-3">
          <span class="min-w-0 flex-1">
            <span class="block truncate font-semibold">{{ profile.name }}</span>
            <span class="block truncate text-xs text-muted">
              {{ profile.server }}<template v-if="profile.user"> · {{ profile.user }}</template
              ><template v-if="profile.notes"> · {{ profile.notes }}</template>
            </span>
          </span>
          <button
            type="button"
            class="h-7 shrink-0 rounded-md border border-stroke bg-card px-3 hover:bg-card-hover"
            @click="edit(profile.id)"
          >
            Edit
          </button>
          <button
            type="button"
            class="h-7 shrink-0 rounded-md border border-stroke bg-card px-3 hover:bg-card-hover"
            @click="removeProfile(profile.id)"
          >
            Remove
          </button>
        </div>
        <ServerShareList
          class="mt-2"
          :profile="profile"
          pick-label="Copy path"
          @pick="copy"
        />
      </li>
    </ul>
    <p
      v-else
      class="mt-2 text-xs text-muted"
    >
      No servers are saved.
    </p>
    <p
      v-if="notice"
      class="mt-2 text-xs text-muted"
      role="status"
    >
      {{ notice }}
    </p>

    <form
      class="mt-3 grid grid-cols-2 gap-2 text-[13px]"
      @submit.prevent="save"
    >
      <label class="flex flex-col gap-1">
        Display name
        <input
          v-model="form.name"
          type="text"
          autocomplete="off"
          placeholder="Main file server"
          :class="field"
        />
      </label>
      <label class="flex flex-col gap-1">
        Server name
        <input
          v-model="form.server"
          type="text"
          autocomplete="off"
          spellcheck="false"
          placeholder="fs01"
          :class="field"
        />
      </label>
      <label class="flex flex-col gap-1">
        User name (optional)
        <input
          v-model="form.user"
          type="text"
          autocomplete="off"
          placeholder="DOMAIN\name"
          :class="field"
        />
      </label>
      <label class="flex flex-col gap-1">
        Notes
        <input
          v-model="form.notes"
          type="text"
          autocomplete="off"
          :class="field"
        />
      </label>
      <div class="col-span-2 flex items-center gap-2">
        <button
          type="submit"
          class="h-8 rounded-md bg-accent px-4 font-semibold text-on-accent hover:bg-accent-hover"
        >
          {{ editing === null ? 'Add server' : 'Save changes' }}
        </button>
        <button
          v-if="editing !== null"
          type="button"
          class="h-8 rounded-md border border-stroke bg-card px-4 hover:bg-card-hover"
          @click="cancel"
        >
          Cancel
        </button>
        <span
          v-if="formError"
          class="text-danger"
          role="alert"
        >
          {{ formError }}
        </span>
      </div>
    </form>
  </section>
</template>
