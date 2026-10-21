import { invoke, isTauri } from '@tauri-apps/api/core'
import type { Candidate, Snapshot, TaskView } from './types'

export const desktop = isTauri()
export const demo = !desktop && new URLSearchParams(window.location.search).get('demo') === '1'

const previewSnapshot: Snapshot = {
  connection: 'ready',
  desktopConnected: false,
  desktopRestartRequired: false,
  connectionMessage: '界面预览 · 未连接真实 Codex',
  codexVersion: '预览',
  accountLabel: null,
  compatibilityMessage: '这些任务与额度仅用于演示界面，不会连接真实会话。',
  pausedAll: false,
  checkedAt: Math.floor(Date.now() / 1000),
  quota: {
    observedAt: Math.floor(Date.now() / 1000), source: 'managed',
    buckets: [{
      id: 'example', label: null, blocked: true,
      windows: [
        { kind: 'primary', durationMins: 300, usedPercent: 100, resetsAt: Math.floor(Date.now() / 1000) + 2292 },
        { kind: 'secondary', durationMins: 10080, usedPercent: 64, resetsAt: Math.floor(Date.now() / 1000) + 309600 },
      ],
    }],
  },
  tasks: [
    {
      id: 'preview-waiting', title: '修复登录后的页面跳转', cwd: '/example/field-notes',
      state: 'waiting_quota', enabled: true, mode: 'managed',
      detail: '任务因额度耗尽中断，等待额度恢复后核对并续跑。',
      resetsAt: Math.floor(Date.now() / 1000) + 2292,
      events: [{ at: Math.floor(Date.now() / 1000) - 900, message: '额度中断 · 5 小时已用 100%，每周已用 64%（示例）。' }],
    },
    {
      id: 'preview-running', title: '补充搜索模块的测试', cwd: '/example/field-notes',
      state: 'completed', enabled: true, mode: 'managed', detail: '最近一轮已正常结束，不会自动发送继续。', resetsAt: null,
      events: [{ at: Math.floor(Date.now() / 1000) - 3600, message: '已加入观察列表。' }],
    },
    {
      id: 'preview-attention', title: '整理组件的使用说明', cwd: '/example/component-library',
      state: 'paused', enabled: false, mode: 'monitor',
      detail: '监测已关闭。', resetsAt: null,
      events: [{ at: Math.floor(Date.now() / 1000) - 1800, message: '已关闭任务监测。' }],
    },
  ],
}

const previewCandidates: Candidate[] = [
  { id: 'preview-candidate', title: '优化文章列表的加载速度', cwd: '/example/field-notes', updatedAt: Math.floor(Date.now() / 1000) - 700, status: 'idle', eligible: false, reason: '可加入监测；不会自动接管原会话。' },
]

function previewCommand<T>(command: string, args?: Record<string, unknown>): T {
  const task = previewSnapshot.tasks.find(item => item.id === args?.threadId)
  if (command === 'get_snapshot') return structuredClone(previewSnapshot) as T
  if (command === 'choose_project') return '/example/new-project' as T
  if (command === 'launch_managed') throw new Error('预览模式不会启动真实 Codex。请在桌面应用中使用。')
  if (command === 'list_candidates') return structuredClone(previewCandidates.filter(candidate => !previewSnapshot.tasks.some(item => item.id === candidate.id))) as T
  if (command === 'set_task_enabled' && task) task.enabled = args?.enabled === true
  if (command === 'set_paused') previewSnapshot.pausedAll = args?.paused === true
  if (command === 'remove_task') previewSnapshot.tasks = previewSnapshot.tasks.filter(item => item.id !== args?.threadId)
  if (command === 'add_task') {
    const candidate = previewCandidates.find(item => item.id === args?.threadId)
    if (candidate && !task) previewSnapshot.tasks.push({ ...candidate, mode: 'monitor', enabled: false, state: 'needs_attention', detail: candidate.reason ?? '', resetsAt: null, events: [{ at: Math.floor(Date.now() / 1000), message: '加入观察列表，等待安全核验。' }] })
  }
  return undefined as T
}

export async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (desktop) return invoke<T>(command, args)
  if (demo) return previewCommand<T>(command, args)
  if (command === 'get_snapshot') return {
    connection: 'missing', connectionMessage: '请在桌面应用中连接本机 Codex。',
    desktopConnected: false,
    desktopRestartRequired: false,
    codexVersion: null, accountLabel: null, compatibilityMessage: null, pausedAll: false, tasks: [],
  } as T
  throw new Error('浏览器无法连接本机 Codex，请打开桌面应用。')
}

export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  if (typeof error === 'string') return error
  return '操作未完成，请重试。'
}

export function projectName(cwd: string): string {
  return cwd.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || cwd || '未指定项目'
}

const stateLabels: Record<string, string> = {
  running: '执行中', waiting_quota: '等待额度', checking: '正在核对',
  needs_attention: '需要处理', completed: '本轮已结束', paused: '已暂停', ready: '观察中',
  reconciling: '正在核对',
}

export function stateLabel(state: string, mode: TaskView['mode'] = 'monitor'): string {
  if (state === 'monitoring') return mode === 'monitor' ? '监测中' : '续跑待命'
  return stateLabels[state] ?? '需要核对'
}

export function formatTime(timestamp: number | null, withDate = false): string {
  if (timestamp === null || !Number.isFinite(timestamp)) return '待确认'
  const date = new Date(timestamp * 1000)
  if (Number.isNaN(date.getTime())) return '待确认'
  return new Intl.DateTimeFormat('zh-CN', {
    ...(withDate ? { month: 'numeric', day: 'numeric' } as const : {}),
    hour: '2-digit', minute: '2-digit', hour12: false,
  }).format(date)
}
