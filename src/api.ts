import { invoke, isTauri } from '@tauri-apps/api/core'
import type { Candidate, Snapshot } from './types'

export const desktop = isTauri()
export const demo = !desktop && new URLSearchParams(window.location.search).get('demo') === '1'

const previewSnapshot: Snapshot = {
  connection: 'ready',
  connectionMessage: '界面预览 · 未连接真实 Codex',
  codexVersion: '预览',
  accountLabel: null,
  compatibilityMessage: '这些任务仅用于演示界面，不会连接真实会话。自动续跑的执行链路尚未实现。',
  pausedAll: false,
  checkedAt: Math.floor(Date.now() / 1000),
  tasks: [
    {
      id: 'preview-waiting', title: '修复登录后的页面跳转', cwd: '/example/field-notes',
      state: 'needs_attention', enabled: true,
      detail: '已开启监测，尚不能确认原客户端的实时执行状态；自动续跑尚未实现。',
      resetsAt: null,
      events: [{ at: Math.floor(Date.now() / 1000) - 900, message: '已读取会话历史，执行状态需要核对。' }],
    },
    {
      id: 'preview-running', title: '补充搜索模块的测试', cwd: '/example/field-notes',
      state: 'completed', enabled: true, detail: '最近一轮已正常结束，不会自动发送继续。', resetsAt: null,
      events: [{ at: Math.floor(Date.now() / 1000) - 3600, message: '已加入观察列表。' }],
    },
    {
      id: 'preview-attention', title: '整理组件的使用说明', cwd: '/example/component-library',
      state: 'paused', enabled: false,
      detail: '监测已关闭。', resetsAt: null,
      events: [{ at: Math.floor(Date.now() / 1000) - 1800, message: '已关闭任务监测。' }],
    },
  ],
}

const previewCandidates: Candidate[] = [
  { id: 'preview-candidate', title: '优化文章列表的加载速度', cwd: '/example/field-notes', updatedAt: Math.floor(Date.now() / 1000) - 700, status: 'idle', eligible: false, reason: '可加入监测；自动续跑尚未实现。' },
]

function previewCommand<T>(command: string, args?: Record<string, unknown>): T {
  const task = previewSnapshot.tasks.find(item => item.id === args?.threadId)
  if (command === 'get_snapshot') return structuredClone(previewSnapshot) as T
  if (command === 'list_candidates') return structuredClone(previewCandidates.filter(candidate => !previewSnapshot.tasks.some(item => item.id === candidate.id))) as T
  if (command === 'set_task_enabled' && task) task.enabled = args?.enabled === true
  if (command === 'set_paused') previewSnapshot.pausedAll = args?.paused === true
  if (command === 'remove_task') previewSnapshot.tasks = previewSnapshot.tasks.filter(item => item.id !== args?.threadId)
  if (command === 'add_task') {
    const candidate = previewCandidates.find(item => item.id === args?.threadId)
    if (candidate && !task) previewSnapshot.tasks.push({ ...candidate, enabled: false, state: 'needs_attention', detail: candidate.reason ?? '', resetsAt: null, events: [{ at: Math.floor(Date.now() / 1000), message: '加入观察列表，等待安全核验。' }] })
  }
  return undefined as T
}

export async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (desktop) return invoke<T>(command, args)
  if (demo) return previewCommand<T>(command, args)
  if (command === 'get_snapshot') return {
    connection: 'missing', connectionMessage: '请在桌面应用中连接本机 Codex。',
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
  monitoring: '观察中', reconciling: '正在核对',
}

export function stateLabel(state: string): string {
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
