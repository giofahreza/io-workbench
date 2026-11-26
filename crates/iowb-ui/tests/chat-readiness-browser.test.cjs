const assert = require('node:assert/strict');
const { test } = require('node:test');
const { openChatScrollPage } = require('./fixtures/chat-scroll-page.cjs');

test('provider setup and durable scoped rejection work in desktop and narrow browsers', {
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
        await page.route('**/api/chat/models?**', route => {
          const gateway = new URL(route.request().url()).searchParams.get('runtime') === 'io_gateway';
          return route.fulfill({ contentType: 'application/json', body: JSON.stringify({
            provider: 'claude', runtime: gateway ? 'io_gateway' : 'native_cli', ready: gateway,
            models: gateway ? [{ value: 'cld:sonnet-test', label: 'Gateway Sonnet' }] : [],
            readiness: { installed: false, nodeAvailable: false, npmAvailable: false, nodeInstallUrl: 'https://nodejs.org/en/download' },
            problem: gateway ? null : { code: 'provider_cli_unavailable', message: 'Claude CLI is missing on the Workbench server.', action: 'install', docsUrl: 'https://code.claude.com/docs/en/setup' },
          }) });
        });
        await page.evaluate(() => loadChatModelsIntoSelect('claude', true));
        assert.equal(await page.locator('#chat-provider-readiness').isVisible(), true);
        assert.match(await page.locator('#chat-provider-readiness').innerText(), /server/);
        assert.equal(await page.locator('#chat-provider-readiness a').count(), 2);
        assert.equal(await page.locator('#chat-provider-readiness button').innerText(), 'Recheck');
        const cardBounds = await page.locator('#chat-provider-readiness').boundingBox();
        assert.ok(cardBounds.x >= 0 && cardBounds.x + cardBounds.width <= width + 1);
        const previousRows = await page.locator('#chat-output [data-chat-history-key]').count();
        await page.evaluate(() => {
          state.chatSetupAttempt = { sessionId: state.chatSessionId, requestId: 'new-attempt', draft: 'Preserve this prompt', projectPath: '/synthetic/project', handled: false };
          document.querySelector('#chat-prompt').value = '';
        });
        fixture.send({ type: 'chat_setup_required', requestId: 'old-attempt', code: 'chat_runtime_mismatch', action: 'new_chat', message: 'Stale error must be ignored' });
        await page.evaluate(() => new Promise(resolve => setTimeout(resolve, 100)));
        assert.equal(await page.locator('#chat-output .chat-setup-card').count(), 0);
        const message = 'Native chat requires a new gateway chat. <img src=x onerror="window.injected=true">';
        const rejection = { type: 'chat_setup_required', requestId: 'new-attempt', code: 'chat_runtime_mismatch', action: 'new_chat', message,
          historyMessage: { id: 'durable-setup-error', role: 'system', content: message, timestamp: '2026-09-30T01:01:00Z',
            metadata: { kind: 'chat_error', phase: 'rejected', code: 'chat_runtime_mismatch', action: 'new_chat' } } };
        fixture.send(rejection);
        const errorRow = page.locator('#chat-output [data-chat-history-key="durable-setup-error"]');
        await errorRow.waitFor();
        assert.equal(await errorRow.locator('img').count(), 0);
        assert.equal(await page.evaluate(() => window.injected), undefined);
        assert.match(await errorRow.innerText(), /Native chat requires a new gateway chat/);
        assert.equal(await page.locator('#chat-prompt').inputValue(), 'Preserve this prompt');
        assert.equal(await page.locator('#chat-output [data-chat-history-key]').count(), previousRows + 1);
        fixture.send(rejection);
        await page.evaluate(() => new Promise(resolve => setTimeout(resolve, 100)));
        assert.equal(await errorRow.count(), 1);
        assert.equal(await page.locator('#chat-output [data-chat-history-key]').count(), previousRows + 1);
        assert.deepEqual(fixture.errors, []);
      } finally { await fixture.close(); }
    }
  } finally { await browser.close(); }
});
