import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { call, desktop } from './api'

export type ThemePreference = 'dark' | 'light' | 'system'
const storageKey = 'autoresume.theme'

function validTheme(value: unknown): ThemePreference {
  return value === 'light' || value === 'system' ? value : 'dark'
}

export function useTheme() {
  let initial: ThemePreference = 'dark'
  try { initial = validTheme(localStorage.getItem(storageKey)) } catch { /* Preferences remain usable without storage. */ }
  const preference = ref<ThemePreference>(initial)
  const media = window.matchMedia('(prefers-color-scheme: dark)')
  const systemDark = ref(media.matches)
  const resolved = computed(() => preference.value === 'system' ? (systemDark.value ? 'dark' : 'light') : preference.value)
  const onMedia = () => { systemDark.value = media.matches }
  const onStorage = (event: StorageEvent) => {
    if (event.key === storageKey) preference.value = validTheme(event.newValue)
  }
  let themeFrame = 0
  watch(resolved, theme => {
    const root = document.documentElement
    cancelAnimationFrame(themeFrame)
    root.classList.add('theme-switching')
    root.dataset.theme = theme
    themeFrame = requestAnimationFrame(() => {
      themeFrame = requestAnimationFrame(() => root.classList.remove('theme-switching'))
    })
  }, { immediate: true })
  watch(preference, value => {
    try { localStorage.setItem(storageKey, value) } catch { /* Apply locally even if saving is unavailable. */ }
    if (desktop) void call('set_window_theme', { theme: value }).catch(() => { /* Web content remains readable if native theme is unavailable. */ })
  }, { immediate: true })
  media.addEventListener('change', onMedia)
  window.addEventListener('storage', onStorage)
  onBeforeUnmount(() => {
    cancelAnimationFrame(themeFrame)
    document.documentElement.classList.remove('theme-switching')
    media.removeEventListener('change', onMedia)
    window.removeEventListener('storage', onStorage)
  })
  return { preference, resolved }
}
