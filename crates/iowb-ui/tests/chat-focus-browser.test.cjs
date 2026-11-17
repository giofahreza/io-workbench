const assert = require('node:assert/strict');
const { test } = require('node:test');
const { openChatScrollPage, settle } = require('./fixtures/chat-scroll-page.cjs');

test('Alt+I focuses the web chat prompt safely', { skip: !process.env.PLAYWRIGHT_MODULE }, async t => {
  const { chromium } = require(process.env.PLAYWRIGHT_MODULE);
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_EXECUTABLE ? { executablePath: process.env.CHROME_EXECUTABLE } : {}),
  });
  t.after(() => browser.close());
  for (const width of [1280, 390]) {
    await t.test(`${width}px`, async () => {
      const fixture = await openChatScrollPage(browser, { width });
      try {
        await fixture.stopPolling();
        const page = fixture.page;
        const focused = () => page.evaluate(() => document.activeElement.id);
        await page.locator('#chat-prompt').fill('Keep this draft');
        await page.evaluate(() => {
          const prompt = document.querySelector('#chat-prompt');
          prompt.setSelectionRange(2, 6);
          prompt.blur();
        });
        const scrollTop = await page.locator('#chat-output').evaluate(node => node.scrollTop);
        await page.keyboard.press('Alt+i');
        assert.equal(await focused(), 'chat-prompt');
        assert.deepEqual(await page.locator('#chat-prompt').evaluate(node => [node.value, node.selectionStart, node.selectionEnd]), ['Keep this draft', 2, 6]);
        assert.equal(await page.locator('#chat-output').evaluate(node => node.scrollTop), scrollTop);
        await page.keyboard.press('Alt+k');
        assert.equal(await page.locator('#chat-session-config-modal').count(), 1);
        await page.evaluate(() => document.querySelector('#chat-prompt').blur());
        await page.keyboard.press('Alt+i');
        assert.notEqual(await focused(), 'chat-prompt');
        await page.evaluate(() => closeChatSessionConfigModal());
        await page.evaluate(() => {
          document.querySelector('#chat-prompt').disabled = true;
        });
        await page.keyboard.press('Alt+i');
        assert.notEqual(await focused(), 'chat-prompt');
        await page.evaluate(() => {
          document.querySelector('#chat-prompt').disabled = false;
        });
        await page.keyboard.press('Control+k');
        await page.locator('#command-search').fill('Focus chat prompt');
        await page.keyboard.press('Enter');
        await page.waitForFunction(() => document.activeElement.id === 'chat-prompt');
        await page.evaluate(async () => { await switchView('files'); document.activeElement.blur(); });
        await page.keyboard.press('Alt+i');
        await page.waitForFunction(() => activeView() === 'chat' && document.activeElement.id === 'chat-prompt');
        await page.evaluate(() => document.activeElement.blur());
        await page.keyboard.press('Alt+Shift+i');
        assert.notEqual(await focused(), 'chat-prompt');
        await page.evaluate(() => document.dispatchEvent(new KeyboardEvent('keydown', {
          key: 'i', code: 'KeyI', altKey: true, isComposing: true, bubbles: true,
        })));
        await settle(page);
        assert.notEqual(await focused(), 'chat-prompt');
        assert.deepEqual(fixture.errors, []);
      } finally {
        await fixture.close();
      }
    });
  }
});
