export interface TaskEvent {
  at: number
  message: string
}

export interface TaskView {
  id: string
  title: string
  cwd: string
  state: string
  enabled: boolean
  detail: string
  resetsAt: number | null
  events: TaskEvent[]
}

export interface Snapshot {
  connection: 'checking' | 'ready' | 'missing' | 'error' | 'login_required'
  connectionMessage: string
  codexVersion: string | null
  accountLabel: string | null
  compatibilityMessage: string | null
  pausedAll: boolean
  checkedAt?: number | null
  tasks: TaskView[]
}

export interface Candidate {
  id: string
  title: string
  cwd: string
  updatedAt: number
  status: string
  eligible: boolean
  reason: string | null
}
