import type { QuotaWindow } from './types'

export function quotaLabel(window: QuotaWindow): string {
  const minutes = window.durationMins
  if (minutes === 10080) return '每周额度'
  if (minutes !== null && Number.isFinite(minutes) && minutes > 0) {
    if (minutes % 1440 === 0) return `${minutes / 1440} 天额度`
    if (minutes % 60 === 0) return `${minutes / 60} 小时额度`
    return `${minutes} 分钟额度`
  }
  return window.kind === 'primary' ? '短窗口额度' : '长窗口额度'
}

export function validPercent(value: number | null): number | null {
  return value !== null && Number.isFinite(value) && value >= 0 && value <= 100 ? value : null
}

export function quotaCountdown(resetsAt: number | null, now: number): string {
  if (resetsAt === null || !Number.isFinite(resetsAt)) return '重置时间未知'
  const seconds = Math.ceil(resetsAt - now)
  if (seconds <= 0) return '等待核对额度'
  const days = Math.floor(seconds / 86400)
  const hours = Math.floor(seconds % 86400 / 3600)
  const minutes = Math.floor(seconds % 3600 / 60)
  if (days) return `${days}天 ${hours}小时后重置`
  if (hours) return `${hours}小时 ${minutes}分后重置`
  return `${minutes}分 ${seconds % 60}秒后重置`
}
