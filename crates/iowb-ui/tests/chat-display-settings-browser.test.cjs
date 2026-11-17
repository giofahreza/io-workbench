const assert = require('node:assert/strict');
const { test } = require('node:test');
const { openChatScrollPage, settle } = require('./fixtures/chat-scroll-page.cjs');

test('explicit display preferences update live sections without replacing or fetching history', {
  skip: !process.env.PLAYWRIGHT_MODULE,
}, async t => {
  const { chromium } = require(process.env.PLAYWRIGHT_MODULE);
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_EXECUTABLE ? { executablePath: process.env.CHROME_EXECUTABLE } : {}),
  });
  t.after(() => browser.close());
  for (const width of [390, 1280]) {
    await t.test(`${width}px`, async () => {
      const f = await openChatScrollPage(browser, { width });
      try {
        await f.stopPolling();
        await f.startStream('### Thinking\n\nKeep these thoughts.\n\n### exec / Parameters\n\n'
          + 'Command parameters.\n\n### Response\n\nVisible answer.');
        const snapshots = f.snapshots().length;
        await f.page.evaluate(() => {
          window.__settingsRow = chatStream.node;
          window.__settingsThinking = chatStream.text.querySelector('details.thinking-section');
          window.__settingsParameters = chatStream.text.querySelector('details.exec-parameters');
          showChatSessionConfigModal();
        });
        await f.page.locator('[data-chat-display-setting="expandThinking"]').uncheck();
        await f.page.locator('[data-chat-display-setting="expandParameters"]').uncheck();
        assert.deepEqual(await f.page.evaluate(() => ({
          thinking: window.__settingsThinking.open, parameters: window.__settingsParameters.open,
        })), { thinking: false, parameters: false });
        await f.page.locator('[data-chat-display-setting="expandThinking"]').check();
        assert.deepEqual(await f.page.evaluate(() => ({
          thinking: window.__settingsThinking.open, parameters: window.__settingsParameters.open,
        })), { thinking: true, parameters: false });
        await f.page.locator('[data-chat-display-setting="autoScrollToBottom"]').check();
        await settle(f.page);
        assert.ok(await f.page.evaluate(() => {
          const output = chatOutputRoot();
          return output.scrollHeight - output.clientHeight - output.scrollTop <= 1;
        }), 'explicitly enabling following reaches the latest output');
        f.send({ type: 'output', content: '\n\nAnother paragraph.', done: false });
        await f.page.waitForFunction(() => state.chatBuffer.includes('Another paragraph.'));
        assert.deepEqual(await f.page.evaluate(() => ({
          sameRow: chatStream.node === window.__settingsRow,
          sameThinking: chatStream.text.querySelector('details.thinking-section') === window.__settingsThinking,
          thinking: window.__settingsThinking.open,
          parameters: window.__settingsParameters.open,
        })), { sameRow: true, sameThinking: true, thinking: true, parameters: false });
        assert.equal(f.snapshots().length, snapshots, 'preference changes need no history fetch');
        assert.deepEqual(f.errors, []);
      } finally {
        await f.close();
      }
    });
  }
});
