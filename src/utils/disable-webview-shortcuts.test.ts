// @vitest-environment jsdom

import { describe, expect, it } from 'vitest'

import { disableWebViewShortcuts } from './disable-webview-shortcuts'

describe('WebView 快捷键保护', () => {
  it('阻止 F6 且不影响普通按键，清理后恢复默认行为', () => {
    const cleanup = disableWebViewShortcuts()
    const f6Event = new KeyboardEvent('keydown', {
      key: 'F6',
      cancelable: true,
    })
    const ordinaryEvent = new KeyboardEvent('keydown', {
      key: 'A',
      cancelable: true,
    })

    document.dispatchEvent(f6Event)
    document.dispatchEvent(ordinaryEvent)

    expect(f6Event.defaultPrevented).toBe(true)
    expect(ordinaryEvent.defaultPrevented).toBe(false)

    cleanup()
    const eventAfterCleanup = new KeyboardEvent('keydown', {
      key: 'F6',
      cancelable: true,
    })
    document.dispatchEvent(eventAfterCleanup)
    expect(eventAfterCleanup.defaultPrevented).toBe(false)
  })
})
