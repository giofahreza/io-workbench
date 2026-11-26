const assert = require('node:assert/strict');
const { test } = require('node:test');
const { openChatScrollPage } = require('./fixtures/chat-scroll-page.cjs');

test('native Claude switching preserves settings and restores an unsent draft at desktop and narrow widths', {
  skip: !process.env.PLAYWRIGHT_MODULE,
}, async () => {
  const { chromium } = require(process.env.PLAYWRIGHT_MODULE);
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROME_EXECUTABLE ? { executablePath: process.env.CHROME_EXECUTABLE } : {}) });
  try {
    for (const width of [1280, 390]) {
      const fixture = await openChatScrollPage(browser, { width });
      const { page } = fixture;
      try {
        fixture.session.runtime = 'native_cli';
        await page.route('**/api/chat/models?**', route => route.fulfill({
          contentType: 'application/json', body: JSON.stringify({
            provider: 'claude', runtime: 'native_cli', pinned: true, ready: true,
            nativeSessionSettings: true, readiness: { installed: true, authState: 'configured_unverified' },
            models: [{ value: '', label: 'Keep native Claude session settings' }, { value: 'sonnet', label: 'Sonnet' }],
          }),
        }));
        await page.evaluate(() => {
          state.preferences.chatCli = 'claude';
          state.preferences.chatProvider = 'claude';
          state.preferences.chatModel = 'sonnet';
          return loadChatModelsIntoSelect('claude', true);
        });
        assert.equal(await page.locator('#chat-model').inputValue(), '');
        assert.equal(await page.locator('#chat-route').inputValue(), 'native_cli');
        assert.equal(await page.locator('#chat-route').isDisabled(), true);
        await page.locator('#chat-prompt').fill('Continue the native Claude chat');
        await page.evaluate(() => document.querySelector('#chat-form').requestSubmit());
        await page.waitForFunction(() => Boolean(state.chatSetupAttempt?.requestId));
        const attempt = await page.evaluate(() => state.chatSetupAttempt);
        await page.waitForTimeout(100);
        const command = fixture.clientMessages.find(message => message.requestId === attempt.requestId);
        assert.equal(command?.type, 'start_session');
        assert.equal(command.sessionId, fixture.session.id);
        assert.equal(command.provider, 'claude');
        assert.equal(command.model, '');
        fixture.send({
          type: 'chat_setup_required', provider: 'claude', requestId: attempt.requestId,
          code: 'claude_send_rejected', action: 'retry', message: 'Claude is busy in the terminal; no message was sent.',
          historyMessage: { id: 'claude-rejection', role: 'system', content: 'Claude is busy in the terminal; no message was sent.',
            timestamp: '2026-11-26T00:00:00Z', metadata: { kind: 'chat_error', phase: 'rejected' } },
        });
        await page.waitForFunction(() => document.querySelector('#chat-prompt').value === 'Continue the native Claude chat');
        assert.equal(await page.evaluate(() => state.chatSessionId), fixture.session.id);
        assert.deepEqual(fixture.errors, []);
      } finally { await fixture.close(); }
    }
  } finally { await browser.close(); }
});
