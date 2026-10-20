<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { listen } from '@tauri-apps/api/event'
import { call, demo, desktop, errorMessage, formatTime, projectName } from './api'
import type { Candidate, Snapshot, TaskView } from './types'
import TaskCard from './components/TaskCard.vue'
import TaskInspector from './components/TaskInspector.vue'
import QuotaCard from './components/QuotaCard.vue'
import { useTheme } from './theme'
import appMark from './assets/icon.svg'
import pauseIcon from './assets/figma/workbench-DarkIconPause.svg'
import expandIcon from './assets/figma/tray-DarkIconExpand.svg'
import settingsIcon from './assets/figma/tray-DarkIconSettings.svg'
import closeIcon from './assets/figma/picker-DarkIconClose.svg'
import searchIcon from './assets/figma/workbench-IconSearch.svg'
import checkIcon from './assets/figma/setup-IconCheck.svg'
import loginAlert from './assets/figma/setup-IconAlert.svg'
import blueDot from './assets/figma/picker-StatusDot.svg'

const tray = new URLSearchParams(window.location.search).get('view') === 'tray'
const snapshot = ref<Snapshot>({
  connection: 'checking', connectionMessage: '正在检查本机 Codex…', codexVersion: null,
  accountLabel: null, compatibilityMessage: null, pausedAll: false, tasks: [],
})
const loading = ref(true)
const error = ref('')
const busy = ref(new Set<string>())
const candidatesDialog = ref<HTMLDialogElement | null>(null)
const connectionDialog = ref<HTMLDialogElement | null>(null)
const capabilityDialog = ref<HTMLDialogElement | null>(null)
const compactDetailDialog = ref<HTMLDialogElement | null>(null)
const appearanceMenu = ref<HTMLDetailsElement | null>(null)
const { preference: themePreference } = useTheme()
const candidateTab = ref<'new' | 'existing'>('new')
const projectPath = ref('')
const autoResume = ref(false)
const launchError = ref('')
const candidates = ref<Candidate[]>([])
const candidatesLoading = ref(false)
const candidatesError = ref('')
const candidateSearch = ref('')
const taskSearch = ref('')
const taskFilter = ref<'all' | 'waiting_quota' | 'needs_attention'>('all')
const selectedTaskId = ref('')
const lastChecked = computed(() => snapshot.value.checkedAt ?? null)
const filteredCandidates = computed(() => {
  const query = candidateSearch.value.trim().toLocaleLowerCase()
  return candidates.value.filter(candidate => `${candidate.title} ${candidate.cwd}`.toLocaleLowerCase().includes(query))
})
const filteredTasks = computed(() => {
  const query = taskSearch.value.trim().toLocaleLowerCase()
  return snapshot.value.tasks.filter(task =>
    (taskFilter.value === 'all' || task.state === taskFilter.value) &&
    `${task.title} ${task.cwd}`.toLocaleLowerCase().includes(query))
})
const selectedTask = computed(() => filteredTasks.value.find(task => task.id === selectedTaskId.value) ?? null)
watch(filteredTasks, tasks => {
  if (!tasks.some(task => task.id === selectedTaskId.value)) selectedTaskId.value = tasks[0]?.id ?? ''
})
const connected = computed(() => snapshot.value.connection === 'ready')
const desktopChatsConnected = computed(() => snapshot.value.tasks.some(task => task.mode === 'desktop'))
const connectionLabel = computed(() => demo ? '预览模式' : ({
  checking: '正在连接', ready: '已连接', missing: '未连接', error: '连接异常', login_required: '需要登录',
})[snapshot.value.connection])
const waitingCount = computed(() => snapshot.value.tasks.filter(task => task.state === 'waiting_quota').length)
const attentionCount = computed(() => snapshot.value.tasks.filter(task => task.state === 'needs_attention').length)
const dialogOpeners = new WeakMap<HTMLDialogElement, HTMLElement>()
let pendingRefresh: Promise<void> | null = null
let disposed = false
let pollTimer: ReturnType<typeof setInterval> | undefined
let unlistenSelection: (() => void) | undefined
let unlistenAddTask: (() => void) | undefined

function dismissAppearance(event: PointerEvent) {
  if (event.target instanceof Node && !appearanceMenu.value?.contains(event.target)) appearanceMenu.value?.removeAttribute('open')
}

function showDialog(dialog: HTMLDialogElement | null) {
  if (!dialog || dialog.open) return
  if (document.activeElement instanceof HTMLElement) dialogOpeners.set(dialog, document.activeElement)
  dialog.showModal()
}

function restoreFocus(event: Event) {
  const dialog = event.target as HTMLDialogElement
  const opener = dialogOpeners.get(dialog)
  if (opener?.isConnected) opener.focus()
}

function refresh() {
  if (disposed) return Promise.resolve()
  if (pendingRefresh) return pendingRefresh
  pendingRefresh = (async () => {
    try {
      const next = await call<Snapshot>('get_snapshot')
      if (!disposed) {
        snapshot.value = next
        if (!next.tasks.some(task => task.id === selectedTaskId.value)) selectedTaskId.value = next.tasks[0]?.id ?? ''
      }
    } catch (cause) {
      if (!disposed) error.value = errorMessage(cause)
    } finally {
      pendingRefresh = null
      loading.value = false
    }
  })()
  return pendingRefresh
}

async function mutate(command: string, args?: Record<string, unknown>, key = command) {
  if (busy.value.has(key)) return false
  busy.value.add(key)
  error.value = ''
  try {
    await call<void>(command, args)
    // Wait for an in-flight observation before fetching the post-mutation state.
    if (pendingRefresh) await pendingRefresh
    await refresh()
    return true
  } catch (cause) {
    error.value = errorMessage(cause)
    return false
  } finally {
    busy.value.delete(key)
  }
}

async function loadCandidates() {
  candidateSearch.value = ''
  candidatesError.value = ''
  candidates.value = []
  candidatesLoading.value = true
  try {
    candidates.value = await call<Candidate[]>('list_candidates')
  } catch (cause) {
    candidatesError.value = errorMessage(cause)
  } finally {
    candidatesLoading.value = false
  }
}

function openCandidates() {
  if (tray) {
    if (desktop) void call<void>('show_main_window', { threadId: null, addTask: true }).catch(cause => { error.value = errorMessage(cause) })
    else {
      const url = new URL(window.location.href)
      url.searchParams.delete('view')
      url.searchParams.set('addTask', '1')
      window.location.assign(url)
    }
    return
  }
  candidateTab.value = 'new'
  projectPath.value = ''
  autoResume.value = false
  launchError.value = ''
  showDialog(candidatesDialog.value)
}

async function showExisting() {
  candidateTab.value = 'existing'
  await loadCandidates()
}

async function chooseProject() {
  launchError.value = ''
  try {
    const selected = await call<string | null>('choose_project')
    if (selected) projectPath.value = selected
  } catch (cause) { launchError.value = errorMessage(cause) }
}

async function launchManaged() {
  if (!projectPath.value.trim() || busy.value.has('launch_managed')) return
  launchError.value = ''
  const result = await mutate('launch_managed', { cwd: projectPath.value.trim(), autoResume: autoResume.value })
  if (result) candidatesDialog.value?.close()
  else {
    launchError.value = error.value
    error.value = ''
  }
}

async function addCandidate(candidate: Candidate) {
  const result = await mutate('add_task', { threadId: candidate.id })
  if (result) {
    taskSearch.value = ''
    taskFilter.value = 'all'
    selectedTaskId.value = candidate.id
    candidatesDialog.value?.close()
  } else candidatesError.value = error.value
}

async function openMain(threadId?: string) {
  if (desktop) {
    try {
      await call<void>('show_main_window', { threadId: threadId ?? null })
    } catch (cause) { error.value = errorMessage(cause) }
  } else {
    const url = new URL(window.location.href)
    url.searchParams.delete('view')
    if (threadId) url.searchParams.set('task', threadId)
    window.location.assign(url)
  }
}

function selectTask(task: TaskView) {
  selectedTaskId.value = task.id
  if (tray) void openMain(task.id)
  else if (window.matchMedia('(max-width: 700px)').matches) showDialog(compactDetailDialog.value)
}

async function removeTask() {
  const id = selectedTask.value?.id
  if (!id) return
  const result = await mutate('remove_task', { threadId: id }, id)
  if (result) compactDetailDialog.value?.close()
}

onMounted(async () => {
  document.addEventListener('pointerdown', dismissAppearance)
  selectedTaskId.value = new URLSearchParams(window.location.search).get('task') ?? ''
  if (desktop && !tray) {
    try {
      const unlisten = await listen<string>('select-task', async event => {
        taskSearch.value = ''
        taskFilter.value = 'all'
        if (pendingRefresh) await pendingRefresh
        await refresh()
        if (!disposed && snapshot.value.tasks.some(task => task.id === event.payload)) selectedTaskId.value = event.payload
      })
      if (disposed) unlisten()
      else unlistenSelection = unlisten
      const stopAddTask = await listen('add-task', openCandidates)
      if (disposed) stopAddTask()
      else unlistenAddTask = stopAddTask
    } catch (cause) { error.value = errorMessage(cause) }
  }
  await refresh()
  if (!tray && new URLSearchParams(window.location.search).get('addTask') === '1') openCandidates()
  // This only refreshes the Rust-owned view; no scheduling or continuation runs in Vue.
  if (!disposed) pollTimer = setInterval(() => { void refresh() }, 3000)
})
onBeforeUnmount(() => {
  disposed = true
  if (pollTimer) clearInterval(pollTimer)
  unlistenSelection?.()
  unlistenAddTask?.()
  document.removeEventListener('pointerdown', dismissAppearance)
})
</script>

<template>
  <div class="application" :class="{ 'tray-view': tray, 'desktop-app': desktop }">
    <div v-if="demo" class="preview-bar"><span>示例数据</span><a :href="tray ? '?view=tray' : '/'">退出预览</a></div>
    <header class="app-header" data-tauri-drag-region>
      <div class="brand" data-tauri-drag-region><img class="brand-icon" :src="appMark" alt=""/><h1 data-tauri-drag-region>{{ tray ? 'AutoResume' : 'Codex AutoResume' }}</h1><span v-if="tray" class="connection-dot" :class="snapshot.connection" :title="connectionLabel" :aria-label="connectionLabel"/></div>
      <div class="header-actions">
        <button v-if="!tray" class="connection-button" :class="snapshot.connection" aria-label="查看连接状态" @click="showDialog(connectionDialog)"><span class="connection-dot" :class="snapshot.connection" aria-hidden="true"/>{{ connectionLabel }}</button>
        <button class="icon-button pause-button" :class="{ active: snapshot.pausedAll }" :aria-label="snapshot.pausedAll ? '恢复全部' : '暂停全部'" :title="snapshot.pausedAll ? '恢复全部' : '暂停全部'" :disabled="!snapshot.tasks.length || busy.has('set_paused')" @click="mutate('set_paused', { paused: !snapshot.pausedAll })"><span v-if="snapshot.pausedAll" class="play-icon" aria-hidden="true"/><img v-else :src="pauseIcon" alt=""/></button>
        <details ref="appearanceMenu" class="appearance-menu" @keydown.esc="appearanceMenu?.removeAttribute('open')">
          <summary class="icon-button" aria-label="外观" title="外观"><svg width="18" height="18" viewBox="0 0 24 24" fill="none" aria-hidden="true"><circle cx="12" cy="12" r="8" stroke="currentColor" stroke-width="1.6"/><path d="M12 4a8 8 0 0 1 0 16Z" fill="currentColor"/></svg></summary>
          <div class="appearance-options" role="group" aria-label="选择外观"><button v-for="option in ([{ value: 'dark', label: '深色' }, { value: 'light', label: '浅色' }, { value: 'system', label: '跟随系统' }] as const)" :key="option.value" :aria-pressed="themePreference === option.value" @click="themePreference = option.value; appearanceMenu?.removeAttribute('open')"><span>{{ option.label }}</span><span v-if="themePreference === option.value" aria-hidden="true">✓</span></button></div>
        </details>
        <button v-if="!tray" class="button primary" :disabled="loading" @click="openCandidates">添加任务</button>
      </div>
    </header>
    <div class="capability-bar">
      <button id="capability-status" class="capability-status" @click="showDialog(capabilityDialog)">{{ desktopChatsConnected ? '原聊天续跑 · 实验连接' : '普通连接仅监测已有会话' }}</button>
      <span v-if="snapshot.pausedAll" class="paused-label" role="status">已全部暂停</span>
    </div>
    <div v-if="error && !candidatesDialog?.open && !connectionDialog?.open" class="error-notice" role="alert"><p>{{ error }}</p><button class="icon-button" aria-label="关闭错误提示" @click="error = ''"><img :src="closeIcon" alt=""/></button></div>
    <main class="workspace">
      <section class="task-sidebar" aria-label="任务列表">
        <QuotaCard :quota="snapshot.quota"/>
        <template v-if="!tray">
          <label class="search-field"><img :src="searchIcon" alt=""/><input v-model="taskSearch" type="search" placeholder="搜索任务或项目" aria-label="搜索任务" autocomplete="off"/></label>
          <div class="task-filters" aria-label="按任务状态筛选">
            <button :class="{ active: taskFilter === 'all' }" :aria-pressed="taskFilter === 'all'" @click="taskFilter = 'all'"><img v-if="taskFilter === 'all'" :src="blueDot" alt=""/>全部 {{ snapshot.tasks.length }}</button>
            <button :class="{ active: taskFilter === 'waiting_quota' }" :aria-pressed="taskFilter === 'waiting_quota'" @click="taskFilter = 'waiting_quota'">等待 {{ waitingCount }}</button>
            <button :class="{ active: taskFilter === 'needs_attention' }" :aria-pressed="taskFilter === 'needs_attention'" @click="taskFilter = 'needs_attention'">需处理 {{ attentionCount }}</button>
          </div>
        </template>
        <div v-else class="queue-heading"><h2>任务 {{ snapshot.tasks.length }}</h2><span>{{ snapshot.tasks.filter(task => task.mode !== 'monitor').length }} 个可续跑</span></div>
        <div v-if="loading" class="empty-state" aria-live="polite"><span class="spinner" aria-hidden="true"/><p>正在连接 Codex…</p></div>
        <div v-else-if="!snapshot.tasks.length" class="empty-state"><span class="empty-mark" aria-hidden="true">＋</span><h2>暂无任务</h2><p>从添加任务开始</p></div>
        <div v-else-if="!filteredTasks.length" class="empty-state"><p>没有匹配的任务</p></div>
        <div v-else class="task-list">
          <TaskCard v-for="task in filteredTasks" :key="task.id" :task="task" :selected="!tray && selectedTaskId === task.id" :busy="busy.has(task.id)" :paused-all="snapshot.pausedAll" @select="selectTask(task)" @toggle="mutate('set_task_enabled', { threadId: task.id, enabled: !task.enabled }, task.id)"/>
        </div>
      </section>
      <div v-if="!tray" class="inspector-container">
        <TaskInspector v-if="selectedTask" :task="selectedTask" :busy="busy.has(selectedTask.id)" :paused-all="snapshot.pausedAll" :checked-at="lastChecked" @remove="removeTask"/>
        <div v-else class="inspector-empty"><img class="brand-icon" :src="appMark" alt=""/><p>选择任务，查看状态与记录</p></div>
      </div>
    </main>
    <footer v-if="tray" class="tray-footer">
      <button class="button secondary" :disabled="loading" @click="openCandidates">添加任务</button>
      <div class="footer-actions"><button class="icon-button" aria-label="打开主窗口" title="打开主窗口" @click="openMain()"><img :src="expandIcon" alt=""/></button><button class="icon-button" aria-label="查看连接状态" title="查看连接状态" @click="showDialog(connectionDialog)"><img :src="settingsIcon" alt=""/></button></div>
    </footer>
    <dialog ref="connectionDialog" class="dialog connection-dialog" aria-labelledby="connection-title" @close="restoreFocus">
      <div class="dialog-header"><h2 id="connection-title">连接 Codex</h2><button class="icon-button" aria-label="关闭连接状态" autofocus @click="connectionDialog?.close()"><img :src="closeIcon" alt=""/></button></div>
      <div class="connection-card"><img v-if="connected && !demo" :src="checkIcon" alt=""/><span v-else class="connection-dot" :class="snapshot.connection" aria-hidden="true"/><strong>{{ connectionLabel }}</strong><span v-if="snapshot.codexVersion" class="version-label">{{ snapshot.codexVersion }}</span></div>
      <div class="connection-message" :class="snapshot.connection"><h3 v-if="snapshot.connection === 'login_required'"><img :src="loginAlert" alt=""/>需要重新登录</h3><p>{{ snapshot.connectionMessage }}</p></div>
      <details class="connection-details"><summary>诊断详情</summary><dl><div v-if="snapshot.accountLabel"><dt>账户</dt><dd>{{ snapshot.accountLabel }}</dd></div><div><dt>最近核对</dt><dd class="mono">{{ formatTime(lastChecked, true) }}</dd></div></dl></details>
      <p v-if="error" class="dialog-error" role="alert">{{ error }}</p>
      <div v-if="!demo" class="dialog-footer connection-actions">
        <a v-if="!desktop" :href="tray ? '?demo=1&view=tray' : '?demo=1'" class="button primary">查看界面预览</a>
        <template v-else>
          <button v-if="snapshot.connection === 'login_required'" class="button primary" :disabled="busy.has('login')" @click="mutate('login')">{{ busy.has('login') ? '正在打开…' : '前往官方登录' }}</button>
          <button v-if="snapshot.connection === 'missing'" class="button primary" :disabled="busy.has('open_download')" @click="mutate('open_download')">安装 Codex</button>
          <button class="button secondary" :disabled="busy.has('reconnect')" @click="mutate('reconnect')">{{ busy.has('reconnect') ? '正在连接…' : '重新检测' }}</button>
        </template>
      </div>
    </dialog>
    <dialog ref="capabilityDialog" class="dialog capability-dialog" aria-labelledby="capability-title" @close="restoreFocus">
      <div class="dialog-header"><h2 id="capability-title">续跑范围</h2><button class="icon-button" aria-label="关闭能力说明" autofocus @click="capabilityDialog?.close()"><img :src="closeIcon" alt=""/></button></div>
      <p class="dialog-description">原聊天通过实验桌面连接接入后，会出现在任务列表中。单独开启开关，才会在本次连接确认额度中断后原地续跑。普通历史列表仍仅监测。</p>
      <p class="capability-reason">{{ snapshot.compatibilityMessage || '托管续跑仍处于测试阶段。遇到审批、用户停止或执行状态不明时会暂停，等待你处理。' }}</p>
    </dialog>
    <dialog ref="candidatesDialog" class="dialog candidate-dialog" aria-labelledby="candidate-title" @close="restoreFocus">
      <div class="dialog-header"><h2 id="candidate-title">添加任务</h2><button class="icon-button" aria-label="关闭任务选择" @click="candidatesDialog?.close()"><img :src="closeIcon" alt=""/></button></div>
      <div class="add-task-tabs" role="tablist" aria-label="添加方式">
        <button role="tab" :aria-selected="candidateTab === 'new'" aria-controls="new-task-panel" @click="candidateTab = 'new'">新建任务</button>
        <button role="tab" :aria-selected="candidateTab === 'existing'" aria-controls="existing-task-panel" @click="showExisting">已有会话</button>
      </div>
      <form v-if="candidateTab === 'new'" id="new-task-panel" class="new-task-form" role="tabpanel" @submit.prevent="launchManaged">
        <label class="project-label" for="project-path">项目文件夹</label>
        <div class="project-picker"><input id="project-path" v-model="projectPath" placeholder="选择项目文件夹" autocomplete="off" spellcheck="false"/><button type="button" class="button secondary" @click="chooseProject">选择</button></div>
        <label class="auto-resume-option"><input v-model="autoResume" type="checkbox"/><span><strong>额度恢复后自动续跑</strong><span>仅对此次新建任务生效</span></span></label>
        <p class="launch-description">将在官方 Codex 终端中开始任务。托管环境独立，首次使用需完成官方登录。</p>
        <p v-if="launchError" class="dialog-error" role="alert">{{ launchError }}</p>
        <div class="dialog-footer launch-actions"><button class="button primary" type="submit" :disabled="!projectPath.trim() || busy.has('launch_managed')">{{ busy.has('launch_managed') ? '正在打开…' : '打开 Codex 终端' }}</button></div>
      </form>
      <section v-else id="existing-task-panel" role="tabpanel">
      <label class="search-field"><img :src="searchIcon" alt=""/><input v-model="candidateSearch" type="search" placeholder="搜索任务或项目" aria-label="搜索本地任务" autocomplete="off"/></label>
      <div class="candidate-toolbar"><span class="filter-label"><img :src="blueDot" alt=""/>全部会话</span><span>最近活动 ↓</span></div>
      <div v-if="candidatesError" class="dialog-error" role="alert">{{ candidatesError }}<button class="text-button" :disabled="candidatesLoading" @click="loadCandidates">重试</button></div>
      <div v-else-if="candidatesLoading" class="candidate-empty" aria-live="polite"><span class="spinner" aria-hidden="true"/><p>正在读取本地任务…</p></div>
      <div v-else-if="!filteredCandidates.length" class="candidate-empty"><p>{{ candidateSearch ? '没有匹配的任务' : '暂无可添加的任务' }}</p></div>
      <div v-else class="candidate-list">
        <article v-for="candidate in filteredCandidates" :key="candidate.id" class="candidate-item">
          <div class="candidate-content"><h3 :title="candidate.title">{{ candidate.title || '未命名任务' }}</h3><p><span :title="candidate.cwd">{{ projectName(candidate.cwd) }}</span><span aria-hidden="true">·</span><time>{{ formatTime(candidate.updatedAt, true) }}</time></p></div>
          <button class="text-button candidate-add" :aria-label="`加入${candidate.title || '未命名任务'}监测列表`" :disabled="busy.has('add_task')" @click="addCandidate(candidate)">加入</button>
        </article>
      </div>
      <p class="candidate-note">加入后默认关闭监测；不会执行原任务。</p>
      </section>
    </dialog>
    <dialog ref="compactDetailDialog" class="dialog compact-detail-dialog" aria-label="任务详情" @close="restoreFocus">
      <button class="icon-button compact-detail-close" aria-label="关闭任务详情" autofocus @click="compactDetailDialog?.close()"><img :src="closeIcon" alt=""/></button>
      <TaskInspector v-if="selectedTask" :task="selectedTask" :busy="busy.has(selectedTask.id)" :paused-all="snapshot.pausedAll" :checked-at="lastChecked" @remove="removeTask"/>
    </dialog>
  </div>
</template>
