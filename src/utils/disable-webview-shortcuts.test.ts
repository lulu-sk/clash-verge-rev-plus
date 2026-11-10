import { afterEach, describe, expect, it, vi } from 'vitest'

import { disableWebViewShortcuts } from './disable-webview-shortcuts'

afterEach(() => vi.unstubAllGlobals())

describe('WebView 快捷键保护', () => {
  it('阻止 F6 且不影响普通按键，清理后恢复默认行为', () => {
    const target = new EventTarget()
    vi.stubGlobal('document', target)
    const cleanup = disableWebViewShortcuts()
    const f6Event = Object.assign(new Event('keydown', { cancelable: true }), {
      key: 'F6',
    })
    const ordinaryEvent = Object.assign(
      new Event('keydown', { cancelable: true }),
      { key: 'A' },
    )

    target.dispatchEvent(f6Event)
    target.dispatchEvent(ordinaryEvent)

    expect(f6Event.defaultPrevented).toBe(true)
    expect(ordinaryEvent.defaultPrevented).toBe(false)

    cleanup()
    const eventAfterCleanup = Object.assign(
      new Event('keydown', { cancelable: true }),
      { key: 'F6' },
    )
    target.dispatchEvent(eventAfterCleanup)
    expect(eventAfterCleanup.defaultPrevented).toBe(false)
  })
})
