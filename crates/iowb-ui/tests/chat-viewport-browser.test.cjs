const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { test } = require('node:test');

test('viewport policy preserves reading intent with and without native anchoring', {
  skip: !process.env.PLAYWRIGHT_MODULE,
}, async t => {
  const { chromium } = require(process.env.PLAYWRIGHT_MODULE);
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_EXECUTABLE ? { executablePath: process.env.CHROME_EXECUTABLE } : {}),
  });
  t.after(() => browser.close());
  for (const width of [390, 1280]) {
    for (const anchoring of ['auto', 'none']) {
      await t.test(`${width}px, overflow-anchor:${anchoring}`, async () => {
        const page = await browser.newPage({ viewport: { width, height: 800 } });
        try {
          await page.setContent(`<style>
            #chat-output { height:300px; overflow:auto; overflow-anchor:${anchoring};
              border:1px solid; width:95%; font:16px/24px sans-serif; }
            .chat-line-assistant { min-height:120px; } p { margin:0; }
          </style><div id="chat-output"></div>`);
          await page.addScriptTag({ content: readFileSync(join(__dirname, '../static/app/chat/history.js'), 'utf8') });
          await page.evaluate(() => {
            window.state = { chatSessionId: 'session', preferences: { chatAutoScrollToBottom: false } };
            window.CHAT_AUTOSCROLL_THRESHOLD_PX = 80;
            window.CHAT_HISTORY_LOAD_THRESHOLD_PX = 80;
            window.qs = selector => document.querySelector(selector);
            window.readJsonStorage = () => null;
            window.activeView = () => 'chat';
            window.renderChatFooter = () => {};
            window.updateChatEmptyState = () => {};
            window.settleViewport = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
            chatOutputRoot().innerHTML = Array.from({ length: 30 }, (_, index) =>
              `<div class="chat-line-assistant" data-chat-history-key="${index}"><p>Row ${index} ${'Word of text. '.repeat(10)}</p></div>`).join('');
            bindChatHistoryLoading(chatOutputRoot());
          });
          const result = await page.evaluate(async () => {
            const output = chatOutputRoot();
            output.scrollTop = 1200;
            await settleViewport();
            const initialTop = output.scrollTop;
            for (let index = 0; index < 5; index++) {
              restoreChatViewport(captureChatViewport());
              await settleViewport();
            }
            const unchangedDrift = output.scrollTop - initialTop;
            const snapshot = captureChatViewport();
            const row = document.createElement('div');
            row.className = 'chat-line-assistant';
            row.textContent = 'New history above the reader';
            output.prepend(row);
            restoreChatViewport(snapshot);
            await settleViewport();
            const prependDrift = chatViewportAnchorDelta(output, snapshot.anchor);
            row.style.height = '250px';
            await settleViewport();
            await settleViewport();
            const delayedDrift = chatViewportAnchorDelta(output, snapshot.anchor);

            state.preferences.chatAutoScrollToBottom = true;
            scrollChatToBottom(true);
            await settleViewport();
            // Even a small upward gesture within the near-bottom threshold
            // pauses following until a downward return or explicit jump.
            output.dispatchEvent(new WheelEvent('wheel', { deltaY: -40, bubbles: true }));
            output.scrollTop -= 40;
            await settleViewport();
            const upwardPaused = !chatViewportController().following;
            const reading = captureChatViewport();
            row.style.height = '350px';
            restoreChatViewport(reading);
            await settleViewport();
            const readingGap = output.scrollHeight - output.clientHeight - output.scrollTop;
            output.dispatchEvent(new WheelEvent('wheel', { deltaY: 80, bubbles: true }));
            output.scrollTop = output.scrollHeight;
            await settleViewport();
            const downwardResumed = chatViewportController().following;
            const following = captureChatViewport();
            output.lastElementChild.style.height = '300px';
            restoreChatViewport(following);
            await settleViewport();
            const followingGap = output.scrollHeight - output.clientHeight - output.scrollTop;
            state.preferences.chatAutoScrollToBottom = false;
            const disabled = captureChatViewport();
            output.lastElementChild.style.height = '500px';
            restoreChatViewport(disabled);
            await settleViewport();
            const disabledGap = output.scrollHeight - output.clientHeight - output.scrollTop;
            return { unchangedDrift, prependDrift, delayedDrift, upwardPaused, readingGap,
              downwardResumed, followingGap, disabledGap };
          });
          for (const key of ['unchangedDrift', 'prependDrift', 'delayedDrift', 'followingGap']) {
            assert.ok(Math.abs(result[key]) < 1, `${key}: ${result[key]}px`);
          }
          assert.equal(result.upwardPaused, true);
          assert.ok(result.readingGap >= 39, 'stream growth does not resume paused following');
          assert.equal(result.downwardResumed, true);
          assert.ok(result.disabledGap > 100, 'disabled auto-scroll remains reading after a previous explicit jump');

          const reflow = await page.evaluate(async () => {
            const output = chatOutputRoot();
            resetChatOutputDom();
            output.innerHTML = `<div class="chat-line-assistant"><p>${Array.from({ length: 1400 }, (_, index) => `word${index}`).join(' ')}</p></div><div style="height:1000px">Tail</div>`;
            output.dispatchEvent(new WheelEvent('wheel', { deltaY: -20, bubbles: true }));
            output.scrollTop = 400;
            await settleViewport();
            const snapshot = captureChatViewport();
            output.firstElementChild.style.width = '70%';
            restoreChatViewport(snapshot);
            await settleViewport();
            const explicitDrift = chatViewportAnchorDelta(output, snapshot.anchor);
            // The next reflow is asynchronous (no transaction wrapper), as
            // with a viewport/composer change or a delayed font/image layout.
            const delayedSnapshot = captureChatViewport();
            output.firstElementChild.style.width = '55%';
            await settleViewport();
            await settleViewport();
            return { hasTextAnchor: !!snapshot.anchor.textNode, explicitDrift,
              delayedDrift: chatViewportAnchorDelta(output, delayedSnapshot.anchor) };
          });
          assert.equal(reflow.hasTextAnchor, true, 'long paragraphs use a visible character, not their off-screen top');
          assert.ok(Math.abs(reflow.explicitDrift) < 1);
          assert.ok(Math.abs(reflow.delayedDrift) < 1, `delayed reflow drift: ${reflow.delayedDrift}`);

          const intent = await page.evaluate(async () => {
            const output = chatOutputRoot();
            state.preferences.chatAutoScrollToBottom = true;
            scrollChatToBottom(true);
            await settleViewport();
            const nested = document.createElement('pre');
            nested.style.cssText = 'height:120px;overflow:auto';
            nested.textContent = 'Scrollable code\n'.repeat(60);
            output.append(nested);
            scrollChatToBottom(true);
            nested.scrollTop = 100;
            nested.dispatchEvent(new WheelEvent('wheel', { deltaY: -40, bubbles: true }));
            nested.scrollTop -= 40;
            const nestedKeptFollowing = chatViewportController().following;
            output.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
            output.scrollTop -= 60;
            await settleViewport();
            const keyPaused = !chatViewportController().following;
            restoreChatViewport(captureChatViewport());
            output.dispatchEvent(new WheelEvent('wheel', { deltaY: -30, bubbles: true }));
            output.scrollTop -= 30;
            const inputTop = output.scrollTop;
            await settleViewport();
            const inputDrift = output.scrollTop - inputTop;
            const reading = captureChatViewport();
            restoreChatViewport(reading); // pending next-frame position repair
            jumpToLatestChatMessage(); // must take precedence over that repair
            await settleViewport();
            const jumpGap = output.scrollHeight - output.clientHeight - output.scrollTop;
            output.scrollTop = 400;
            await settleViewport();
            restoreChatViewport(captureChatViewport());
            state.preferences.chatAutoScrollToBottom = false;
            resetChatOutputDom(); // must cancel old repair even in the same session
            output.innerHTML = '<div style="height:2000px">New transcript</div>';
            output.scrollTop = 0;
            await settleViewport();
            const resetTop = output.scrollTop;
            const previousSession = captureChatViewport();
            restoreChatViewport(previousSession); // repair is queued before the session changes
            state.chatSessionId = 'other-session';
            output.scrollTop = 200;
            await settleViewport();
            const pendingSessionTop = output.scrollTop;
            restoreChatViewport(previousSession);
            await settleViewport();
            return { nestedKeptFollowing, keyPaused, inputDrift, jumpGap, resetTop,
              pendingSessionTop, otherSessionTop: output.scrollTop };
          });
          assert.equal(intent.nestedKeptFollowing, true, 'scrolling code does not change transcript intent');
          assert.equal(intent.keyPaused, true, 'keyboard upward scrolling pauses following');
          assert.ok(Math.abs(intent.inputDrift) < 1, 'fresh reader input wins over a queued position repair');
          assert.ok(intent.jumpGap < 1, 'explicit jump wins over pending reading correction');
          assert.equal(intent.resetTop, 0, 'same-session deliberate reset cancels pending corrections');
          assert.equal(intent.pendingSessionTop, 200, 'queued correction cannot move the next session');
          assert.equal(intent.otherSessionTop, 200, 'stale session snapshot cannot move the new session');
        } finally {
          await page.close();
        }
      });
    }
  }
});
