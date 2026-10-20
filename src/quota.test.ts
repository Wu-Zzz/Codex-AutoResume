import { describe, expect, it } from 'vitest'
import { quotaCountdown, quotaLabel, validPercent } from './quota'

describe('honest quota telemetry', () => {
  it('derives names from actual window lengths', () => {
    expect(quotaLabel({ kind: 'primary', durationMins: 180, usedPercent: 20, resetsAt: null })).toBe('3 小时额度')
    expect(quotaLabel({ kind: 'secondary', durationMins: 10080, usedPercent: null, resetsAt: null })).toBe('每周额度')
    expect(quotaLabel({ kind: 'primary', durationMins: null, usedPercent: null, resetsAt: null })).toBe('短窗口额度')
  })
  it('does not interpret missing or malformed percentages as unused quota', () => {
    for (const value of [null, NaN, Infinity, -1, 101]) expect(validPercent(value)).toBeNull()
    expect(validPercent(0)).toBe(0)
    expect(validPercent(100)).toBe(100)
  })
  it('never treats elapsed reset time as proof that quota recovered', () => {
    expect(quotaCountdown(100, 101)).toBe('等待核对额度')
    expect(quotaCountdown(null, 101)).toBe('重置时间未知')
    expect(quotaCountdown(2301, 9)).toBe('38分 12秒后重置')
  })
})
