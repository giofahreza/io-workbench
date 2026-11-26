const assert = require('node:assert/strict');
const { test } = require('node:test');
const { openChatScrollPage } = require('./fixtures/chat-scroll-page.cjs');

test('native Codex continuation preserves settings, session identity and rejected draft in both layouts', {
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
        fixture.session.provider = 'codex';
        fixture.session.runtime = 'native_cli';
        await page.route('**/api/chat/models?**', route => route.fulfill({
          contentType: 'application/json', body: JSON.stringify({
            provider: 'codex', runtime: 'native_cli', pinned: true, ready: true,
            nativeSessionSettings: true, readiness: { installed: true, authState: 'configured_unverified' },
            models: [{ value: '', label: 'Keep native Codex session settings' },
              { value: 'another-model', label: 'Another model' }],
          }),
        }));
        await page.evaluate(() => {
          state.preferences.chatCli = 'codex';
          state.preferences.chatProvider = 'codex';
          state.preferences.chatModel = 'another-model';
          return loadChatModelsIntoSelect('codex', true);
        });
        assert.equal(await page.locator('#chat-model').inputValue(), '');
        assert.equal(await page.locator('#chat-route').inputValue(), 'native_cli');
        assert.equal(await page.locator('#chat-route').isDisabled(), true);
        const historyRows = await page.locator('#chat-output [data-chat-history-key]').count();
        await page.locator('#chat-prompt').fill('Continue this native chat');
        await page.evaluate(() => document.querySelector('#chat-form').requestSubmit());
        await page.waitForFunction(() => Boolean(state.chatSetupAttempt?.requestId));
        const attempt = await page.evaluate(() => state.chatSetupAttempt);
        await page.waitForTimeout(100);
        const command = fixture.clientMessages.find(message => message.requestId === attempt.requestId);
        assert.equal(command?.type, 'start_session');
        assert.equal(command.sessionId, fixture.session.id);
        assert.equal(command.provider, 'codex');
        assert.equal(command.model, '');
        fixture.send({
          type: 'chat_setup_required', provider: 'codex', requestId: attempt.requestId,
          code: 'codex_thread_owned', action: 'retry', message: 'Exit the private CLI or reconnect it to the native daemon.',
          historyMessage: { id: 'native-writer-rejection', role: 'system',
            content: 'Exit the private CLI or reconnect it to the native daemon.',
            timestamp: '2026-11-26T00:00:00Z', metadata: { kind: 'chat_error', phase: 'rejected' } },
        });
        await page.waitForFunction(() => document.querySelector('#chat-prompt').value === 'Continue this native chat');
        assert.equal(await page.evaluate(() => state.chatSessionId), fixture.session.id);
        assert.equal(await page.locator('#chat-output [data-chat-history-key]').count(), historyRows + 1);
        assert.deepEqual(fixture.errors, []);
      } finally {
        await fixture.close();
      }
    }
  } finally {
    await browser.close();
  }
});
