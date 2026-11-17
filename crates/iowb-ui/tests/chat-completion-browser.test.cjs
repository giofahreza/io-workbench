const assert = require('node:assert/strict');
const { test } = require('node:test');
const { openChatScrollPage, settle, waitForRequests } = require('./fixtures/chat-scroll-page.cjs');

test('delayed completion usage stays attached to its own response', {
  skip: !process.env.PLAYWRIGHT_MODULE,
  timeout: 90000,
}, async t => {
  const { chromium } = require(process.env.PLAYWRIGHT_MODULE);
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_EXECUTABLE ? { executablePath: process.env.CHROME_EXECUTABLE } : {}),
  });
  t.after(() => browser.close());

  for (const width of [390, 1280]) {
    for (const nextTurn of ['none', 'streaming', 'completed']) {
      await t.test(`${width}px: next turn ${nextTurn}`, { timeout: 15000 }, async () => {
        const f = await openChatScrollPage(browser, { width });
        const usageRequests = [];
        try {
          await f.stopPolling();
          f.hold(); // Keep completion reconciliation from replacing the local messages.
          await f.page.route('**/token-usage?*', async route => {
            let release;
            const result = new Promise(resolve => { release = resolve; });
            usageRequests.push({ release });
            await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(await result) });
          });
          await f.page.evaluate(() => { state.preferences.chatCli = 'codex'; });
          async function startTurn(number) {
            await f.page.evaluate(number => {
              state.preferences.chatFast = number === 1;
              state.preferences.chatMode = 'bypass';
              document.querySelector('#chat-mode').value = 'bypass';
            }, number);
            const prompt = `Prompt for response ${number}.`;
            await f.page.locator('#chat-prompt').fill(prompt);
            await f.page.locator('#chat-form').evaluate(form => form.requestSubmit());
            await f.page.waitForFunction(prompt => chatHistoryWindow.messages.some(message => message.content === prompt), prompt);
            f.session.active = true;
            f.send({ type: 'session_status', status: 'running', responseId: `usage-response-${number}`, provider: 'codex' });
            // The provider envelope requires the original prompt when computing copy text.
            const content = `user\n${prompt}\nassistant\nResponse ${number} body.`;
            f.send({ type: 'output', content, done: false, responseId: `usage-response-${number}`, provider: 'codex' });
            await f.page.waitForFunction(content => state.chatBuffer === content, content);
            await settle(f.page);
            await f.page.evaluate(number => setChatFastRequested(number !== 1), number);
          }
          async function finishTurn(number) {
            f.session.active = false;
            f.send({ type: 'output', content: '', done: true, responseId: `usage-response-${number}`, provider: 'codex' });
            f.send({ type: 'session_status', status: 'completed', responseId: `usage-response-${number}`, provider: 'codex' });
            await f.page.waitForFunction(() => !chatSessionIsLive(state.chatSessionId));
            await waitForRequests(() => usageRequests, number);
          }
          await startTurn(1);
          await finishTurn(1);
          const original = await f.page.evaluate(() => {
            window.__completedUsageNode = chatStream.node;
            const message = chatHistoryWindow.messages.at(-1);
            return { id: message.id, metadata: message.metadata };
          });
          assert.equal(original.metadata.fast, true, 'completion retains submitted fast despite toggle change');
          if (nextTurn !== 'none') {
            await startTurn(2);
            if (nextTurn === 'streaming') {
              await f.page.evaluate(() => preserveActiveChatStreamSnapshot());
            }
            if (nextTurn === 'completed') {
              await finishTurn(2);
              usageRequests[1].release({ used: 222, breakdown: { input: 200, output: 22 } });
              await f.page.waitForFunction(() => getSessionOverridesFor(state.chatSessionId).tokenUsageDetails?.used === 222);
            }
          }
          const before = await f.page.evaluate(() => ({
            overrides: getSessionOverridesFor(state.chatSessionId),
            live: cachedChatSession(state.chatSessionId).live,
            status: cachedChatSession(state.chatSessionId).status,
            streamingBuffer: cachedChatSession(state.chatSessionId).streamingBuffer,
            nodeHtml: chatStream.node.innerHTML,
          }));
          if (nextTurn === 'streaming') assert.match(before.streamingBuffer, /Response 2 body\./);
          usageRequests[0].release({ used: 111, breakdown: { input: 100, output: 11 } });
          await f.page.waitForFunction(id => chatHistoryWindow.messages.find(message => message.id === id)?.metadata?.tokenUsageDetails?.used === 111, original.id);
          const after = await f.page.evaluate(id => {
            const cached = cachedChatSession(state.chatSessionId);
            return {
              overrides: getSessionOverridesFor(state.chatSessionId),
              live: cached.live,
              status: cached.status,
              streamingBuffer: cached.streamingBuffer,
              nodeHtml: chatStream.node.innerHTML,
              originalConnected: window.__completedUsageNode.isConnected,
              originalFooter: window.__completedUsageNode.querySelector('[data-chat-line-footer-details]')._iowbMetadata,
              originalCopy: window.__completedUsageNode.querySelector('[data-chat-copy-kind="assistant"]')._iowbCopyText,
              messageMetadata: chatHistoryWindow.messages.find(message => message.id === id).metadata,
              cachedMetadata: cached.messages.find(message => message.id === id).metadata,
            };
          }, original.id);
          assert.equal(after.originalConnected, true);
          assert.equal(after.originalFooter.tokenUsageDetails?.used, 111, 'original response receives its usage');
          assert.equal(after.originalFooter.fast, true, 'late usage preserves original fast setting');
          assert.match(after.originalConnected && await f.page.evaluate(() => window.__completedUsageNode.querySelector('.chat-line-footer').textContent), /bypass · fast/);
          assert.equal(after.originalCopy, 'Response 1 body.', 'copy text retains the original buffer and prompt');
          assert.equal(after.messageMetadata.sentAt, original.metadata.sentAt, 'per-message metadata retains the original turn');
          assert.deepEqual(after.cachedMetadata, after.messageMetadata, 'usage is persisted on the matching cached message');
          assert.equal(after.live, before.live, 'usage cannot change the current live state');
          assert.equal(after.status, before.status, 'usage cannot change the current status');
          assert.equal(after.streamingBuffer, before.streamingBuffer, 'usage cannot erase the current cached stream');
          if (nextTurn === 'none') {
            assert.equal(after.overrides.tokenUsageDetails?.used, 111, 'unchanged turn still persists session usage');
          } else {
            assert.deepEqual(after.overrides, before.overrides, 'old usage cannot overwrite newer session metadata');
            assert.equal(after.nodeHtml, before.nodeHtml, 'old usage cannot change the newer response DOM');
          }
          assert.deepEqual(f.errors, [], 'no runtime/resource errors');
        } finally {
          for (const request of usageRequests) request.release({});
          await f.close();
        }
      });
    }
  }
});
