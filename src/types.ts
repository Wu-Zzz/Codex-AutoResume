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
  mode: 'monitor' | 'managed' | 'desktop'
  canEnable?: boolean
}

export interface QuotaWindow {
  kind: 'primary' | 'secondary'
  durationMins: number | null
  usedPercent: number | null
  resetsAt: number | null
}

export interface QuotaTelemetry {
  observedAt: number
  source: 'local' | 'managed' | 'desktop'
  buckets: {
    id: string
    label: string | null
    windows: QuotaWindow[]
    blocked: boolean
  }[]
}

export interface Snapshot {
  connection: 'checking' | 'ready' | 'missing' | 'error' | 'login_required'
  desktopConnected: boolean
  desktopRestartRequired: boolean
  connectionMessage: string
  codexVersion: string | null
  accountLabel: string | null
  compatibilityMessage: string | null
  pausedAll: boolean
  checkedAt?: number | null
  tasks: TaskView[]
  quota?: QuotaTelemetry | null
}

export interface DesktopSetup {
  available: boolean
  installed: boolean
  message: string
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
