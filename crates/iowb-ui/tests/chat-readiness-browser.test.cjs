const assert = require('node:assert/strict');
const { test } = require('node:test');
const { openChatScrollPage } = require('./fixtures/chat-scroll-page.cjs');

test('provider setup and scoped new-chat recovery work in desktop and narrow browsers', {
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
        fixture.send({ type: 'chat_setup_required', requestId: 'new-attempt', code: 'chat_runtime_mismatch', action: 'new_chat', message: 'Native chat requires a new gateway chat. <img src=x onerror="window.injected=true">' });
        await page.locator('#chat-output .chat-setup-card').waitFor();
        assert.equal(await page.locator('#chat-output .chat-setup-card img').count(), 0);
        assert.equal(await page.evaluate(() => window.injected), undefined);
        assert.equal(await page.locator('#chat-prompt').inputValue(), 'Preserve this prompt');
        assert.equal(await page.locator('#chat-output [data-chat-history-key]').count(), previousRows);
        fixture.send({ type: 'chat_setup_required', requestId: 'new-attempt', code: 'chat_runtime_mismatch', action: 'new_chat', message: 'Duplicate' });
        await page.evaluate(() => new Promise(resolve => setTimeout(resolve, 100)));
        assert.equal(await page.locator('#chat-output .chat-setup-card').count(), 1);
        await page.locator('#chat-output .chat-setup-card button').click();
        await page.waitForFunction(() => state.chatSessionId === '' && state.newChatRuntime === 'io_gateway');
        assert.equal(await page.locator('#chat-prompt').inputValue(), 'Preserve this prompt');
        assert.deepEqual(fixture.errors, []);
      } finally { await fixture.close(); }
    }
  } finally { await browser.close(); }
});
