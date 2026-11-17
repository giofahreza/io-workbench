const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { test } = require('node:test');

test('prepend preserves existing DOM, response boundaries, and viewport anchor', {
  skip: !process.env.PLAYWRIGHT_MODULE,
}, async () => {
  const { chromium } = require(process.env.PLAYWRIGHT_MODULE);
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_EXECUTABLE ? { executablePath: process.env.CHROME_EXECUTABLE } : {}),
  });
  try {
    for (const width of [390, 1280]) {
      const page = await browser.newPage({ viewport: { width, height: 800 } });
      await page.setContent(`<style>
        #chat-output { height: 300px; overflow: auto; overflow-anchor: none; }
        .chat-line-user, .chat-line-assistant { min-height: 100px; }
      </style><div id="chat-output"></div>`);
      for (const file of ['render.js', 'history.js', 'stream.js']) {
        await page.addScriptTag({ content: readFileSync(join(__dirname, '../static/app/chat', file), 'utf8') });
      }
      const result = await page.evaluate(() => {
        window.state = { chatSessionId: 'session', preferences: {} };
        window.CHAT_AUTOSCROLL_THRESHOLD_PX = 160;
        window.chatDisplaySettings = () => ({ autoScrollToBottom: false });
        window.qs = (selector) => document.querySelector(selector);
        window.firstDefined = (...values) => values.find((value) => value !== undefined && value !== null);
        window.normalizeMessageMeta = (meta) => meta;
        window.getSessionOverridesFor = () => ({});
        window.syncChatPromptHistoryFromMessages = () => {};
        window.findChatSession = () => ({ id: 'session', provider: 'claude' });
        window.renderChatBubbleHtml = (text) => `<details open><summary>Details</summary>${text}</details>`;
        window.renderChatLineFooter = (node, meta) => { node.textContent = meta ? 'footer' : ''; };
        window.withoutChatToolTelemetrySections = (text) => text;
        window.sidebarProviderIcon = () => 'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==';
        window.sidebarProviderLabel = (provider) => provider;
        window.selectedRunningChatSession = () => null;
        window.scrollChatToBottom = () => {};
        const existing = [
          { id: 'boundary', role: 'assistant', content: 'Existing response' },
          { id: 'user', role: 'user', content: 'Next prompt' },
          { id: 'response', role: 'assistant', content: 'Next response' },
          { id: 'last-user', role: 'user', content: 'Last prompt' },
          { id: 'last-response', role: 'assistant', content: 'Last response' },
        ];
        replayChatMessages(existing);
        const output = chatOutputRoot();
        const previousNodes = Array.from(output.children);
        const details = previousNodes[0].querySelector('details');
        details.open = false;
        const live = document.createElement('div');
        live.textContent = 'Live stream';
        output.append(live);
        output.scrollTop = 30;
        const anchor = captureChatHistoryAnchor();
        prependChatHistoryPage([
          { id: 'older-user', role: 'user', content: 'Original prompt' },
          { id: 'older-response', role: 'assistant', content: 'Beginning of response' },
        ], existing);
        restoreChatHistoryAnchor(anchor);
        const anchorOffset = anchor.node.getBoundingClientRect().top - output.getBoundingClientRect().top;
        return {
          reused: previousNodes.every((node) => node.isConnected && node.parentNode === output),
          liveRetained: output.lastElementChild === live,
          detailsRetained: previousNodes[0].querySelector('details') === details && !details.open,
          boundaryHeaderRemoved: !previousNodes[0].querySelector('.chat-response-header'),
          olderHeaderPresent: !!output.querySelector('[data-chat-history-key="older-response"] .chat-response-header'),
          responseCopy: previousNodes[0].querySelector('[data-chat-copy-kind="assistant"]')._iowbCopyText,
          keys: Array.from(output.children).map((node) => node.dataset.chatHistoryKey).filter(Boolean),
          anchorDrift: Math.abs(anchorOffset - anchor.offset),
        };
      });
      assert.equal(result.reused, true);
      assert.equal(result.liveRetained, true);
      assert.equal(result.detailsRetained, true);
      assert.equal(result.boundaryHeaderRemoved, true);
      assert.equal(result.olderHeaderPresent, true);
      assert.equal(result.responseCopy, 'Beginning of response\n\nExisting response');
      assert.deepEqual(result.keys, ['older-user', 'older-response', 'boundary', 'user', 'response', 'last-user', 'last-response']);
      assert.ok(result.anchorDrift < 1, `anchor drift at width ${width}: ${result.anchorDrift}`);
      await page.close();
    }
  } finally {
    await browser.close();
  }
});
