<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { call, demo, desktop, errorMessage, formatTime, projectName, stateLabel } from './api'
import type { Candidate, Snapshot } from './types'

const snapshot = ref<Snapshot>({
  connection: 'checking', connectionMessage: '正在检查本机 Codex…', codexVersion: null,
  accountLabel: null, compatibilityMessage: null, pausedAll: false, tasks: [],
})
const loading = ref(true)
const error = ref('')
const lastChecked = ref<number | null>(null)
const busy = ref(new Set<string>())
const expanded = ref(new Set<string>())
const dialog = ref<HTMLDialogElement | null>(null)
const connectionDialog = ref<HTMLDialogElement | null>(null)
const capabilityDialog = ref<HTMLDialogElement | null>(null)
const candidates = ref<Candidate[]>([])
const candidatesLoading = ref(false)
const candidatesError = ref('')
const search = ref('')
const selectedId = ref('')
const filteredCandidates = computed(() => {
  const query = search.value.trim().toLocaleLowerCase()
  return candidates.value.filter(candidate => `${candidate.title} ${candidate.cwd}`.toLocaleLowerCase().includes(query))
})
const selected = computed(() => filteredCandidates.value.find(candidate => candidate.id === selectedId.value))
const connected = computed(() => snapshot.value.connection === 'ready')
const connectionLabel = computed(() => demo ? '预览模式' : ({ checking: '正在连接', ready: 'Codex 已连接', missing: '未连接', error: '连接异常', login_required: '需要登录' })[snapshot.value.connection])
let refreshing = false
let disposed = false
let pollTimer: ReturnType<typeof setInterval> | undefined

async function refresh() {
  if (refreshing || disposed) return
  refreshing = true
  try {
    const next = await call<Snapshot>('get_snapshot')
    if (!disposed) {
      snapshot.value = next
      lastChecked.value = next.checkedAt ?? null
    }
  } catch (cause) {
    if (!disposed) error.value = errorMessage(cause)
  } finally {
    refreshing = false
    loading.value = false
  }
}

async function mutate(command: string, args?: Record<string, unknown>, key = command) {
  if (busy.value.has(key)) return false
  busy.value.add(key)
  error.value = ''
  try {
    await call<void>(command, args)
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
  search.value = ''
  selectedId.value = ''
  candidatesError.value = ''
  candidates.value = []
  if (!dialog.value?.open) dialog.value?.showModal()
  candidatesLoading.value = true
  try {
    candidates.value = await call<Candidate[]>('list_candidates')
  } catch (cause) {
    candidatesError.value = errorMessage(cause)
  } finally {
    candidatesLoading.value = false
  }
}

async function addSelected() {
  if (!selected.value || busy.value.has('add_task')) return
  const result = await mutate('add_task', { threadId: selected.value.id })
  if (result) dialog.value?.close()
  else candidatesError.value = error.value
}

function toggleDetails(id: string) {
  if (expanded.value.has(id)) expanded.value.delete(id)
  else expanded.value.add(id)
}

onMounted(async () => {
  await refresh()
  if (!disposed) pollTimer = setInterval(() => { void refresh() }, 3000)
})
onBeforeUnmount(() => {
  disposed = true
  if (pollTimer) clearInterval(pollTimer)
})
</script>

<template>
  <div class="application">
    <div v-if="demo" class="preview-bar"><span>界面预览 · 示例数据</span><a href="/">退出预览 <span aria-hidden="true">↗</span></a></div>
    <header class="app-header">
      <div class="brand">
        <div class="brand-symbol" aria-hidden="true"><svg viewBox="0 0 32 32" fill="none"><path d="M24.8 14a9 9 0 1 0-1.9 7.8" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"/><path d="m20 10 5 4 2-6" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/><path d="m13.5 12 6 4-6 4v-8Z" fill="currentColor"/></svg></div>
        <h1>Codex <strong>AutoResume</strong></h1>
      </div>
      <button class="connection-button" :class="snapshot.connection" aria-label="查看连接状态" @click="connectionDialog?.showModal()"><i aria-hidden="true" />{{ connectionLabel }}<svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="m6 4 4 4-4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg></button>
    </header>

    <main>
      <div v-if="error" class="error-notice" role="alert"><p>{{ error }}</p><button class="icon-button" aria-label="关闭错误提示" @click="error = ''">×</button></div>

      <section class="tasks-section" aria-labelledby="tasks-title">
        <div class="section-heading">
          <div class="section-title"><h2 id="tasks-title">任务</h2><span class="count">{{ snapshot.tasks.length }}</span></div>
          <div class="task-actions">
            <button v-if="snapshot.tasks.length" class="button secondary pause-button" :disabled="busy.has('set_paused')" @click="mutate('set_paused', { paused: !snapshot.pausedAll })"><svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path v-if="snapshot.pausedAll" d="m6 3 6 5-6 5V3Z" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/><path v-else d="M5 3v10M11 3v10" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/></svg>{{ snapshot.pausedAll ? '恢复监测' : '暂停全部' }}</button>
            <button class="button primary add-button" :disabled="!connected || loading" @click="openCandidates"><svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/></svg>添加任务</button>
          </div>
        </div>

        <button id="capability-status" class="capability-status" @click="capabilityDialog?.showModal()"><span class="capability-copy"><svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><circle cx="8" cy="8" r="5.8" stroke="currentColor" stroke-width="1.2"/><path d="M8 7v4M8 4.8v.2" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/></svg><span>仅监测<span class="capability-separator">·</span>自动续跑尚未开放</span></span><svg class="chevron" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="m6 4 4 4-4 4" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/></svg></button>
        <div v-if="snapshot.pausedAll" class="paused-banner" role="status">已暂停监测</div>

        <div v-if="loading" class="empty-state" aria-live="polite"><span class="spinner" aria-hidden="true"/><h3>正在连接 Codex…</h3></div>
        <div v-else-if="!snapshot.tasks.length" class="empty-state">
          <div class="empty-symbol" aria-hidden="true"><svg viewBox="0 0 48 48" fill="none"><rect x="9" y="7" width="30" height="34" rx="6" stroke="currentColor" stroke-width="1.5"/><path d="M17 17h14M17 24h14M17 31h8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg></div>
          <h3>暂无任务</h3>
          <p>{{ connected ? '添加一个本地 Codex 任务' : '请先连接本机 Codex' }}</p>
        </div>
        <div v-else class="task-list">
          <div class="list-labels" aria-hidden="true"><span>任务 / 项目</span><span>状态</span><span>自动续跑</span></div>
          <article v-for="task in snapshot.tasks" :key="task.id" class="task-item" :class="{ expanded: expanded.has(task.id) }">
            <div class="task-row">
              <button class="task-summary" :aria-expanded="expanded.has(task.id)" :aria-controls="`details-${task.id}`" @click="toggleDetails(task.id)">
                <svg class="task-chevron" :class="{ rotated: expanded.has(task.id) }" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="m6 4 4 4-4 4" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/></svg>
                <span class="task-name"><strong :title="task.title">{{ task.title || '未命名任务' }}</strong><span :title="task.cwd">{{ projectName(task.cwd) }}</span></span>
              </button>
              <div class="task-status"><span class="status-label" :class="task.state"><i aria-hidden="true"/>{{ stateLabel(task.state) }}</span><span v-if="task.resetsAt !== null" class="reset-time">{{ formatTime(task.resetsAt, true) }} 后核对</span></div>
              <div class="toggle-cell"><button class="switch" :class="{ on: task.enabled }" role="switch" :aria-checked="task.enabled" :aria-label="`${task.title}自动续跑偏好`" aria-describedby="capability-status" :title="task.enabled ? '已保存开启偏好；当前仅监测' : '自动续跑偏好已关闭'" :disabled="busy.has(task.id)" @click="mutate('set_task_enabled', { threadId: task.id, enabled: !task.enabled }, task.id)"><span/></button></div>
            </div>
            <div v-if="expanded.has(task.id)" :id="`details-${task.id}`" class="task-details">
              <p class="task-detail-message">{{ task.detail }}</p>
              <ol v-if="task.events.length" class="activity-list"><li v-for="(event, eventIndex) in task.events.slice(-6).reverse()" :key="`${event.at}-${eventIndex}`"><time>{{ formatTime(event.at, true) }}</time><span>{{ event.message }}</span></li></ol>
              <div class="detail-footer"><span :title="task.cwd">{{ task.cwd }}</span><button class="text-button remove-button" :disabled="busy.has(task.id)" @click="mutate('remove_task', { threadId: task.id }, task.id)">移除任务</button></div>
            </div>
          </article>
        </div>
      </section>
    </main>

    <dialog ref="connectionDialog" class="dialog connection-dialog" aria-labelledby="connection-title">
      <div class="dialog-header"><h2 id="connection-title">连接状态</h2><button class="icon-button" aria-label="关闭连接状态" autofocus @click="connectionDialog?.close()">×</button></div>
      <div class="connection-summary"><span class="connection-orb" :class="snapshot.connection" aria-hidden="true"/><h3>{{ connectionLabel }}</h3></div>
      <p class="dialog-description">{{ snapshot.connectionMessage }}</p>
      <dl class="connection-details"><div v-if="snapshot.codexVersion"><dt>Codex 版本</dt><dd>{{ snapshot.codexVersion }}</dd></div><div v-if="snapshot.accountLabel"><dt>账户</dt><dd>{{ snapshot.accountLabel }}</dd></div><div><dt>最近核对</dt><dd>{{ formatTime(lastChecked, true) }}</dd></div></dl>
      <p v-if="error" class="dialog-error" role="alert">{{ error }}</p>
      <div v-if="desktop || (!desktop && !demo)" class="dialog-footer connection-actions">
        <a v-if="!desktop && !demo" href="?demo=1" class="button primary">查看界面预览 <span aria-hidden="true">↗</span></a>
        <template v-else>
          <button v-if="snapshot.connection === 'login_required'" class="button primary" :disabled="busy.has('login')" @click="mutate('login')">{{ busy.has('login') ? '正在打开…' : '登录 Codex' }}</button>
          <button v-if="snapshot.connection === 'missing'" class="button primary" :disabled="busy.has('open_download')" @click="mutate('open_download')">安装 Codex <span aria-hidden="true">↗</span></button>
          <button class="button secondary" :disabled="busy.has('reconnect')" @click="mutate('reconnect')">{{ busy.has('reconnect') ? '正在连接…' : '重新连接' }}</button>
        </template>
      </div>
    </dialog>

    <dialog ref="capabilityDialog" class="dialog capability-dialog" aria-labelledby="capability-title">
      <div class="dialog-header"><h2 id="capability-title">当前仅支持监测</h2><button class="icon-button" aria-label="关闭能力说明" autofocus @click="capabilityDialog?.close()">×</button></div>
      <p class="dialog-description">自动续跑开关仅保存偏好，当前版本不会启动或恢复任务。</p>
      <p v-if="snapshot.compatibilityMessage" class="capability-reason">{{ snapshot.compatibilityMessage }}</p>
      <p v-else class="capability-reason">安全接管原会话的能力尚未验证。请在 Codex 中继续任务。</p>
    </dialog>

    <dialog ref="dialog" class="dialog candidate-dialog" aria-labelledby="candidate-title">
      <div class="dialog-header"><h2 id="candidate-title">添加本地任务</h2><button class="icon-button" aria-label="关闭任务选择" @click="dialog?.close()">×</button></div>
      <label class="search-field"><svg viewBox="0 0 20 20" fill="none" aria-hidden="true"><circle cx="8.5" cy="8.5" r="5.5" stroke="currentColor" stroke-width="1.5"/><path d="m13 13 4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg><input v-model="search" type="search" placeholder="搜索任务或项目" aria-label="搜索任务或项目" autocomplete="off" autofocus/></label>
      <div v-if="candidatesError" class="dialog-error" role="alert">{{ candidatesError }}<button class="text-button" :disabled="candidatesLoading" @click="openCandidates">重试</button></div>
      <div v-else-if="candidatesLoading" class="candidate-empty" aria-live="polite"><span class="spinner" aria-hidden="true"/><p>正在读取本地任务…</p></div>
      <div v-else-if="!filteredCandidates.length" class="candidate-empty"><p>{{ search ? '没有匹配的任务' : '暂无可添加的任务' }}</p></div>
      <div v-else class="candidate-list" role="radiogroup" aria-label="本地任务"><label v-for="candidate in filteredCandidates" :key="candidate.id" class="candidate-item" :class="{ selected: selectedId === candidate.id }"><input v-model="selectedId" type="radio" name="candidate" :value="candidate.id"/><span class="candidate-content"><span class="candidate-name">{{ candidate.title || '未命名任务' }}</span><span class="candidate-meta"><span :title="candidate.cwd">{{ projectName(candidate.cwd) }}</span><time>{{ formatTime(candidate.updatedAt, true) }}</time></span></span></label></div>
      <div class="dialog-footer candidate-footer"><details v-if="selected && !selected.eligible" class="candidate-reason"><summary>仅加入监测</summary><p>{{ selected.reason || '自动续跑尚未开放。' }}</p></details><button class="button primary" :disabled="!selected || candidatesLoading || busy.has('add_task')" @click="addSelected">{{ busy.has('add_task') ? '正在添加…' : '添加所选' }}</button></div>
    </dialog>
  </div>
</template>
