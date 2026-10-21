<script setup lang="ts">
import type { Component } from 'vue'
import { nextTick, onBeforeUnmount, onMounted, ref } from 'vue'

export interface MenuItem {
  label: string
  icon?: Component
  disabled?: boolean
  action: () => void
}

/** `null` draws a separator. */
const props = defineProps<{ items: (MenuItem | null)[]; x: number; y: number }>()
const emit = defineEmits<{ close: [] }>()

const menu = ref<HTMLElement | null>(null)
const position = ref({ left: props.x, top: props.y })

function buttons(): HTMLButtonElement[] {
  return [...(menu.value?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])]
}

function onKeydown(event: KeyboardEvent): void {
  const all = buttons()
  const index = all.indexOf(document.activeElement as HTMLButtonElement)
  if (event.key === 'Escape' || event.key === 'Tab') {
    event.preventDefault()
    emit('close')
  } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault()
    const step = event.key === 'ArrowDown' ? 1 : -1
    all.at((index + step + all.length) % all.length)?.focus()
  } else if (event.key === 'Home' || event.key === 'End') {
    event.preventDefault()
    ;(event.key === 'Home' ? all.at(0) : all.at(-1))?.focus()
  }
}

function choose(item: MenuItem): void {
  emit('close')
  item.action()
}

function onPointerDown(event: PointerEvent): void {
  if (!menu.value?.contains(event.target as Node)) {
    emit('close')
  }
}
const close = (): void => emit('close')

onMounted(async () => {
  await nextTick()
  // Keep the menu on screen.
  const box = menu.value?.getBoundingClientRect()
  if (box) {
    position.value = {
      left: Math.max(4, Math.min(props.x, window.innerWidth - box.width - 4)),
      top: Math.max(4, Math.min(props.y, window.innerHeight - box.height - 4)),
    }
  }
  buttons().at(0)?.focus()
  document.addEventListener('pointerdown', onPointerDown, true)
  window.addEventListener('blur', close)
  window.addEventListener('resize', close)
})
onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', onPointerDown, true)
  window.removeEventListener('blur', close)
  window.removeEventListener('resize', close)
})
</script>

<template>
  <Teleport to="body">
    <div
      ref="menu"
      role="menu"
      class="fixed z-50 min-w-56 rounded-lg border border-stroke-strong bg-app p-1 text-[13px] shadow-lg"
      :style="{ left: `${String(position.left)}px`, top: `${String(position.top)}px` }"
      @keydown="onKeydown"
      @contextmenu.prevent
    >
      <template
        v-for="(item, index) in items"
        :key="index"
      >
        <div
          v-if="item === null"
          role="separator"
          class="mx-1 my-1 h-px bg-stroke"
        />
        <button
          v-else
          type="button"
          role="menuitem"
          class="flex h-8 w-full items-center gap-2.5 rounded px-2.5 text-left outline-none hover:bg-subtle-strong focus-visible:bg-subtle-strong disabled:text-faint disabled:hover:bg-transparent"
          :disabled="item.disabled"
          @click="choose(item)"
        >
          <component
            :is="item.icon"
            v-if="item.icon"
            class="size-4 shrink-0"
            :stroke-width="1.75"
          />
          <span
            v-else
            class="size-4 shrink-0"
          />
          {{ item.label }}
        </button>
      </template>
    </div>
  </Teleport>
</template>
