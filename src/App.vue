<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { listen } from '@tauri-apps/api/event'
import { call, demo, desktop, errorMessage, formatTime, projectName } from './api'
import type { Candidate, Snapshot, TaskView } from './types'
import TaskCard from './components/TaskCard.vue'
import TaskInspector from './components/TaskInspector.vue'
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

async function openCandidates() {
  candidateSearch.value = ''
  candidatesError.value = ''
  candidates.value = []
  showDialog(candidatesDialog.value)
  candidatesLoading.value = true
  try {
    candidates.value = await call<Candidate[]>('list_candidates')
  } catch (cause) {
    candidatesError.value = errorMessage(cause)
  } finally {
    candidatesLoading.value = false
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
    } catch (cause) { error.value = errorMessage(cause) }
  }
  await refresh()
  // This only refreshes the Rust-owned view; no scheduling or continuation runs in Vue.
  if (!disposed) pollTimer = setInterval(() => { void refresh() }, 3000)
})
onBeforeUnmount(() => {
  disposed = true
  if (pollTimer) clearInterval(pollTimer)
  unlistenSelection?.()
})
</script>

<template>
  <div class="application" :class="{ 'tray-view': tray, 'desktop-app': desktop }">
    <div v-if="demo" class="preview-bar"><span>示例数据</span><a :href="tray ? '?view=tray' : '/'">退出预览</a></div>
    <header class="app-header" data-tauri-drag-region>
      <div class="brand" data-tauri-drag-region><img class="brand-icon" :src="appMark" alt=""/><h1 data-tauri-drag-region>{{ tray ? 'AutoResume' : 'Codex AutoResume' }}</h1><span v-if="tray" class="connection-dot" :class="snapshot.connection" :title="connectionLabel" :aria-label="connectionLabel"/></div>
      <div class="header-actions">
        <button v-if="!tray" class="connection-button" :class="snapshot.connection" aria-label="查看连接状态" @click="showDialog(connectionDialog)"><span class="connection-dot" :class="snapshot.connection" aria-hidden="true"/>{{ connectionLabel }}</button>
        <button class="icon-button pause-button" :class="{ active: snapshot.pausedAll }" :aria-label="snapshot.pausedAll ? '恢复全部监测' : '暂停全部监测'" :title="snapshot.pausedAll ? '恢复全部监测' : '暂停全部监测'" :disabled="!snapshot.tasks.length || busy.has('set_paused')" @click="mutate('set_paused', { paused: !snapshot.pausedAll })"><span v-if="snapshot.pausedAll" class="play-icon" aria-hidden="true"/><img v-else :src="pauseIcon" alt=""/></button>
        <button v-if="!tray" class="button primary" :disabled="!connected || loading" @click="openCandidates">添加任务</button>
      </div>
    </header>
    <div class="capability-bar">
      <button id="capability-status" class="capability-status" @click="showDialog(capabilityDialog)">仅监测<span aria-hidden="true">·</span>自动续跑未开放</button>
      <span v-if="snapshot.pausedAll" class="paused-label" role="status">监测已暂停</span>
    </div>
    <div v-if="error && !candidatesDialog?.open && !connectionDialog?.open" class="error-notice" role="alert"><p>{{ error }}</p><button class="icon-button" aria-label="关闭错误提示" @click="error = ''"><img :src="closeIcon" alt=""/></button></div>
    <main class="workspace">
      <section class="task-sidebar" aria-label="监测任务">
        <template v-if="!tray">
          <label class="search-field"><img :src="searchIcon" alt=""/><input v-model="taskSearch" type="search" placeholder="搜索任务或项目" aria-label="搜索监测任务" autocomplete="off"/></label>
          <div class="task-filters" aria-label="按任务状态筛选">
            <button :class="{ active: taskFilter === 'all' }" :aria-pressed="taskFilter === 'all'" @click="taskFilter = 'all'"><img v-if="taskFilter === 'all'" :src="blueDot" alt=""/>全部 {{ snapshot.tasks.length }}</button>
            <button :class="{ active: taskFilter === 'waiting_quota' }" :aria-pressed="taskFilter === 'waiting_quota'" @click="taskFilter = 'waiting_quota'">等待 {{ waitingCount }}</button>
            <button :class="{ active: taskFilter === 'needs_attention' }" :aria-pressed="taskFilter === 'needs_attention'" @click="taskFilter = 'needs_attention'">需处理 {{ attentionCount }}</button>
          </div>
        </template>
        <div v-else class="queue-heading"><h2>任务 {{ snapshot.tasks.length }}</h2><span>监测</span></div>
        <div v-if="loading" class="empty-state" aria-live="polite"><span class="spinner" aria-hidden="true"/><p>正在连接 Codex…</p></div>
        <div v-else-if="!snapshot.tasks.length" class="empty-state"><span class="empty-mark" aria-hidden="true">＋</span><h2>暂无任务</h2><p>{{ connected ? '添加一个本地 Codex 任务' : '请先连接本机 Codex' }}</p></div>
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
      <button class="button secondary" :disabled="!connected || loading" @click="openCandidates">添加任务</button>
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
      <div class="dialog-header"><h2 id="capability-title">当前仅支持监测</h2><button class="icon-button" aria-label="关闭能力说明" autofocus @click="capabilityDialog?.close()"><img :src="closeIcon" alt=""/></button></div>
      <p class="dialog-description">开关控制本工具的任务监测。自动续跑尚未实现，当前版本不会启动或恢复 Codex 任务。</p>
      <p class="capability-reason">{{ snapshot.compatibilityMessage || '安全接管原会话的能力尚未验证。请在 Codex 中继续任务。' }}</p>
    </dialog>
    <dialog ref="candidatesDialog" class="dialog candidate-dialog" aria-labelledby="candidate-title" @close="restoreFocus">
      <div class="dialog-header"><h2 id="candidate-title">添加本地任务</h2><button class="icon-button" aria-label="关闭任务选择" @click="candidatesDialog?.close()"><img :src="closeIcon" alt=""/></button></div>
      <label class="search-field"><img :src="searchIcon" alt=""/><input v-model="candidateSearch" type="search" placeholder="搜索任务或项目" aria-label="搜索本地任务" autocomplete="off" autofocus/></label>
      <div class="candidate-toolbar"><span class="filter-label"><img :src="blueDot" alt=""/>全部会话</span><span>最近活动 ↓</span></div>
      <div v-if="candidatesError" class="dialog-error" role="alert">{{ candidatesError }}<button class="text-button" :disabled="candidatesLoading" @click="openCandidates">重试</button></div>
      <div v-else-if="candidatesLoading" class="candidate-empty" aria-live="polite"><span class="spinner" aria-hidden="true"/><p>正在读取本地任务…</p></div>
      <div v-else-if="!filteredCandidates.length" class="candidate-empty"><p>{{ candidateSearch ? '没有匹配的任务' : '暂无可添加的任务' }}</p></div>
      <div v-else class="candidate-list">
        <article v-for="candidate in filteredCandidates" :key="candidate.id" class="candidate-item">
          <div class="candidate-content"><h3 :title="candidate.title">{{ candidate.title || '未命名任务' }}</h3><p><span :title="candidate.cwd">{{ projectName(candidate.cwd) }}</span><span aria-hidden="true">·</span><time>{{ formatTime(candidate.updatedAt, true) }}</time></p></div>
          <button class="text-button candidate-add" :aria-label="`加入${candidate.title || '未命名任务'}监测列表`" :disabled="busy.has('add_task')" @click="addCandidate(candidate)">加入</button>
        </article>
      </div>
      <p class="candidate-note">加入后默认关闭监测；不会执行原任务。</p>
    </dialog>
    <dialog ref="compactDetailDialog" class="dialog compact-detail-dialog" aria-label="任务详情" @close="restoreFocus">
      <button class="icon-button compact-detail-close" aria-label="关闭任务详情" autofocus @click="compactDetailDialog?.close()"><img :src="closeIcon" alt=""/></button>
      <TaskInspector v-if="selectedTask" :task="selectedTask" :busy="busy.has(selectedTask.id)" :paused-all="snapshot.pausedAll" :checked-at="lastChecked" @remove="removeTask"/>
    </dialog>
  </div>
</template>
