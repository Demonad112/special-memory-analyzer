<script setup lang="ts">
import { LoaderCircle } from '@lucide/vue'
import { ref } from 'vue'
import { appAdmin, canDelete, removeItem, type PickedItem } from '@/lib/cleanup'
import { errorMessage } from '@/lib/errors'
import { formatBytes, plural } from '@/lib/format'

const emit = defineEmits<{
  /** A message for the page's toast. */
  notice: [message: string]
  /** The item is gone; the page refreshes what it shows. */
  removed: []
}>()

// Recycle / delete, after a confirm dialog with the size.
const confirmEl = ref<HTMLDialogElement | null>(null)
const pending = ref<{ item: PickedItem; permanent: boolean } | null>(null)
const deleting = ref(false)

/** Asks before deleting `item`; shows a notice instead when permanent delete is not allowed. */
function show(item: PickedItem | null, permanent: boolean): void {
  if (!item || deleting.value) {
    return
  }
  if (!canDelete(1, permanent, appAdmin.elevated)) {
    emit('notice', 'Deleting permanently needs administrator. Use Restart as administrator first.')
    return
  }
  pending.value = { item, permanent }
  confirmEl.value?.showModal()
}

async function confirmDelete(): Promise<void> {
  const request = pending.value
  if (!request) {
    return
  }
  deleting.value = true
  try {
    await removeItem(request.item.id, request.permanent)
    emit(
      'notice',
      `${request.permanent ? 'Deleted' : 'Moved to the Recycle Bin:'} ${request.item.name} (${formatBytes(request.item.size)})`,
    )
    emit('removed')
  } catch (err) {
    emit('notice', errorMessage(err))
  } finally {
    deleting.value = false
    confirmEl.value?.close()
    pending.value = null
  }
}

/** Whether the dialog is up, so a page shortcut can stay out of its way. */
function isOpen(): boolean {
  return Boolean(confirmEl.value?.open)
}

defineExpose({ show, isOpen })
</script>

<template>
  <dialog
    ref="confirmEl"
    class="m-auto w-[min(28rem,90vw)] rounded-lg border border-stroke-strong bg-app p-5 text-fg shadow-xl backdrop:bg-fg/20"
    @close="pending = null"
  >
    <template v-if="pending">
      <h2 class="font-display text-lg font-semibold">
        {{ pending.permanent ? 'Delete permanently?' : 'Move to the Recycle Bin?' }}
      </h2>
      <p class="mt-2 text-[13px] break-all">
        <span class="font-semibold">{{ pending.item.name }}</span>
      </p>
      <p class="mt-1 text-[13px] text-muted">
        {{ formatBytes(pending.item.size) }}
        <template v-if="pending.item.isFolder">
          · {{ plural(pending.item.files, 'file') }}</template
        >
      </p>
      <p
        v-if="pending.permanent"
        class="mt-3 text-[13px] text-danger"
      >
        This skips the Recycle Bin and cannot be undone.
      </p>
      <div class="mt-5 flex justify-end gap-2">
        <button
          type="button"
          class="h-8 rounded-md border border-stroke bg-card px-4 hover:bg-card-hover"
          :disabled="deleting"
          @click="confirmEl?.close()"
        >
          Cancel
        </button>
        <button
          type="button"
          class="flex h-8 items-center gap-1.5 rounded-md px-4 font-semibold disabled:opacity-60"
          :class="
            pending.permanent
              ? 'bg-danger text-on-accent'
              : 'bg-accent text-on-accent hover:bg-accent-hover'
          "
          :disabled="deleting"
          @click="confirmDelete"
        >
          <LoaderCircle
            v-if="deleting"
            class="size-4 animate-spin"
          />
          {{ pending.permanent ? 'Delete' : 'Recycle' }}
        </button>
      </div>
    </template>
  </dialog>
</template>
