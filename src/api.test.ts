import { beforeEach, describe, expect, it, vi } from 'vitest'

const invoke = vi.fn()
const isTauri = vi.fn(() => false)
vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri }))

beforeEach(() => {
  vi.resetModules()
  vi.clearAllMocks()
  isTauri.mockReturnValue(false)
  vi.stubGlobal('window', { location: { search: '' } })
})

describe('desktop boundary', () => {
  it('does not expose fake live tasks or allow browser mutations', async () => {
    const api = await import('./api')
    expect(await api.call('get_snapshot')).toMatchObject({ connection: 'missing', tasks: [] })
    await expect(api.call('add_task', { threadId: 'real-thread' })).rejects.toThrow('桌面应用')
    expect(invoke).not.toHaveBeenCalled()
  })

  it('forwards the original thread ID and enabled intent without scheduling locally', async () => {
    isTauri.mockReturnValue(true)
    const api = await import('./api')
    await api.call('set_task_enabled', { threadId: 'existing-id', enabled: true })
    expect(invoke).toHaveBeenCalledExactlyOnceWith('set_task_enabled', { threadId: 'existing-id', enabled: true })
  })

  it('ignores the demo parameter inside the real desktop', async () => {
    isTauri.mockReturnValue(true)
    vi.stubGlobal('window', { location: { search: '?demo=1' } })
    const api = await import('./api')
    expect(api.demo).toBe(false)
    await api.call('get_snapshot')
    expect(invoke).toHaveBeenCalledExactlyOnceWith('get_snapshot', undefined)
  })

  it('does not retry an uncertain desktop mutation', async () => {
    isTauri.mockReturnValue(true)
    invoke.mockRejectedValueOnce(new Error('connection lost after submission'))
    const api = await import('./api')
    await expect(api.call('set_task_enabled', { threadId: 'existing-id', enabled: true })).rejects.toThrow('connection lost')
    expect(invoke).toHaveBeenCalledTimes(1)
  })

  it('keeps explicit preview changes isolated from the real backend', async () => {
    vi.stubGlobal('window', { location: { search: '?demo=1' } })
    const api = await import('./api')
    await api.call('set_paused', { paused: true })
    expect(await api.call('get_snapshot')).toMatchObject({ pausedAll: true })
    expect(invoke).not.toHaveBeenCalled()
  })
})

describe('portable view formatting', () => {
  it('handles both platform paths and preserves unknown-state uncertainty', async () => {
    const api = await import('./api')
    expect(api.projectName('C:\\Users\\name\\project\\')).toBe('project')
    expect(api.projectName('/Users/name/project/')).toBe('project')
    expect(api.stateLabel('future-state')).toBe('需要核对')
    expect(api.formatTime(Number.NaN)).toBe('待确认')
    expect(api.formatTime(null)).toBe('待确认')
  })
})
