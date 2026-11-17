const assert = require('node:assert/strict');
const { test } = require('node:test');
const {
  openChatScrollPage, readingPosition, rememberReadingPosition, settle, syntheticMessage, waitForRequests,
} = require('./fixtures/chat-scroll-page.cjs');

function assertStableReading(result, label) {
  assert.equal(result.anchorConnected, true, `${label}: anchor DOM retained`);
  assert.equal(result.allRowsRetained, true, `${label}: existing message DOM retained`);
  assert.ok(Math.abs(result.drift) <= 1, `${label}: reading position drifted ${result.drift}px`);
}

test('full web chat keeps its transcript and reading position stable', {
  skip: !process.env.PLAYWRIGHT_MODULE,
  timeout: 180000,
}, async t => {
  const { chromium } = require(process.env.PLAYWRIGHT_MODULE);
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_EXECUTABLE ? { executablePath: process.env.CHROME_EXECUTABLE } : {}),
  });
  t.after(() => browser.close());

  for (const width of [390, 1280]) {
    async function scenario(name, callback, options = {}) {
      await t.test(`${width}px: ${name}`, { timeout: 25000 }, async () => {
        const fixture = await openChatScrollPage(browser, { width, ...options });
        try {
          await callback(fixture);
          assert.deepEqual(fixture.errors, [], 'no runtime/resource errors');
        } finally {
          await fixture.close();
        }
      });
    }

    await scenario('actual two-second polling does not replace rows or drift', async f => {
      f.session.active = true;
      f.send({ type: 'session_status', status: 'running' });
      await f.page.waitForFunction(() => chatSessionIsLive(state.chatSessionId));
      const before = await rememberReadingPosition(f.page, 1200);
      const initialCount = f.snapshots().length;
      await waitForRequests(() => f.snapshots().filter(request => request.completedAt), initialCount + 3);
      await settle(f.page);
      const result = await readingPosition(f.page);
      assertStableReading(result, 'three natural polls');
      assert.equal(result.top, before.top, 'unchanged content retains exact scrollTop');
      assert.equal(result.controlRetained, true, 'older-message button remains the same DOM element');
    });

    await scenario('prepending at scrollTop zero retains the visible message', async f => {
      await f.stopPolling();
      await rememberReadingPosition(f.page, 0);
      await f.page.locator('[data-chat-load-older]').click();
      await f.page.waitForFunction(() => chatHistoryWindow.messages.length === 60);
      assertStableReading(await readingPosition(f.page), 'prepend at top');
    });

    await scenario('tail refresh retains all previously loaded pages and appends new messages', async f => {
      await f.stopPolling();
      await f.page.evaluate(() => { document.querySelector('#chat-output').scrollTop = 0; });
      await f.page.locator('[data-chat-load-older]').click();
      await f.page.waitForFunction(() => chatHistoryWindow.messages.length === 60);
      await rememberReadingPosition(f.page, 1200);
      for (let index = 60; index < 64; index++) f.messages.push(syntheticMessage(index));
      f.session.messageCount = f.messages.length;
      const ok = await f.page.evaluate(() => loadChatHistoryForSession(state.chatSessionId));
      assert.equal(ok, true);
      const result = await readingPosition(f.page);
      assertStableReading(result, 'append to loaded history');
      assert.deepEqual(result.keys, Array.from({ length: 64 }, (_, index) => `m${index}`));
      assert.ok(f.snapshots().every(request => Number(new URLSearchParams(request.query).get('limit')) <= 30),
        'tail refresh remains bounded instead of redownloading loaded history');
    });

    await scenario('a tail gap larger than a page is filled without dropping the reader', async f => {
      await f.stopPolling();
      await rememberReadingPosition(f.page, 1200);
      for (let index = 60; index < 100; index++) f.messages.push(syntheticMessage(index));
      f.session.messageCount = f.messages.length;
      assert.equal(await f.page.evaluate(() => loadChatHistoryForSession(state.chatSessionId)), true);
      const result = await readingPosition(f.page);
      assertStableReading(result, 'forty new messages between polls');
      assert.deepEqual(result.keys, Array.from({ length: 70 }, (_, index) => `m${index + 30}`));
      assert.ok(f.olderRequests().length >= 2, 'the missing range was fetched');
      assert.ok(f.olderRequests().every(request => Number(new URLSearchParams(request.query).get('limit')) <= 30),
        'gap filling uses bounded requests');
    });

    await scenario('history deletion at a page boundary removes stale rows and retains surviving identities', async f => {
      await f.stopPolling();
      await rememberReadingPosition(f.page, 1200);
      f.messages.splice(29, 2);
      f.session.messageCount = f.messages.length;
      assert.equal(await f.page.evaluate(() => loadChatHistoryForSession(state.chatSessionId)), true);
      const result = await readingPosition(f.page);
      assert.equal(result.anchorConnected, true, 'visible surviving message remains connected');
      assert.ok(Math.abs(result.drift) <= 1, `surviving reading position drifted ${result.drift}px`);
      assert.equal(result.keys.includes('m30'), false, 'deleted loaded boundary message is removed');
      const firstIndex = f.messages.findIndex(message => message.id === result.keys[0]);
      assert.ok(firstIndex >= 0 && firstIndex <= 28, 'reconciled range covers the surviving loaded history');
      assert.deepEqual(result.keys, f.messages.slice(firstIndex).map(message => message.id),
        'reconciled messages are contiguous, unique, and in server order');
      assert.equal(await f.page.evaluate(() => chatHistoryWindow.totalCount), 58);
    });

    await scenario('reader movement during a pending request is not undone', async f => {
      await f.stopPolling();
      await rememberReadingPosition(f.page, 1200);
      const requestCount = f.snapshots().length;
      f.hold();
      await f.page.evaluate(() => { window.__pendingLoad = loadChatHistoryForSession(state.chatSessionId); });
      await waitForRequests(() => f.snapshots(), requestCount + 1);
      const moved = await rememberReadingPosition(f.page, 2400);
      f.release();
      assert.equal(await f.page.evaluate(() => window.__pendingLoad), true);
      const after = await readingPosition(f.page);
      assertStableReading(after, 'in-flight reader movement');
      assert.equal(after.top, moved.top);
    });

    await scenario('late replies from the previous session cannot replace the selected conversation', async f => {
      await f.stopPolling();
      const other = f.addSession('other-session');
      const requestCount = f.snapshots().length;
      f.hold(`snapshot:${f.session.id}`);
      await f.page.evaluate(() => { window.__previousSessionLoad = loadChatHistoryForSession(state.chatSessionId); });
      await waitForRequests(() => f.snapshots(), requestCount + 1);
      await f.page.evaluate(session => pickChatSession(session.id, session.projectPath), other.session);
      await f.page.waitForFunction(() => state.chatSessionId === 'other-session' && chatHistoryWindow.sessionId === 'other-session');
      f.release(`snapshot:${f.session.id}`);
      assert.equal(await f.page.evaluate(() => window.__previousSessionLoad), false, 'late load is rejected');
      assert.deepEqual(await f.page.evaluate(() => [...document.querySelectorAll('[data-chat-history-key]')].map(node => node.dataset.chatHistoryKey)),
        other.messages.map(message => message.id));
      assert.equal(await f.page.evaluate(() => state.chatSessionId), other.session.id);
    });

    await scenario('a delayed image above the reader does not move the visible message', async f => {
      await f.stopPolling();
      f.hold('image');
      f.messages[31].content += '\n\n![Synthetic delayed image](/test/delayed-image.svg)';
      assert.equal(await f.page.evaluate(() => loadChatHistoryForSession(state.chatSessionId)), true);
      await rememberReadingPosition(f.page, 2400);
      f.release('image');
      await f.page.waitForFunction(() => {
        const image = document.querySelector('img[alt="Synthetic delayed image"]');
        return image?.complete && image.naturalHeight === 440;
      });
      assertStableReading(await readingPosition(f.page), 'delayed image');
    });

    await scenario('a held snapshot cannot rewind newer live text', async f => {
      await f.stopPolling();
      await f.startStream('Before snapshot.\n\n');
      const requestCount = f.snapshots().length;
      f.hold();
      await f.page.evaluate(() => { window.__pendingLoad = loadChatHistoryForSession(state.chatSessionId); });
      await waitForRequests(() => f.snapshots(), requestCount + 1);
      f.send({ type: 'output', content: 'DURING_REQUEST_NEW_TEXT', done: false });
      await f.page.waitForFunction(() => state.chatBuffer.includes('DURING_REQUEST_NEW_TEXT'));
      const beforeRelease = await f.page.evaluate(() => state.chatBuffer);
      f.release();
      assert.equal(await f.page.evaluate(() => window.__pendingLoad), true);
      assert.equal(await f.page.evaluate(() => state.chatBuffer), beforeRelease);
      assert.equal(await f.page.evaluate(() => state.chatOutputBuffersBySession[state.chatSessionId]), beforeRelease);
      assert.match(await f.page.locator('#chat-output').innerText(), /DURING_REQUEST_NEW_TEXT/);
    });

    await scenario('a stale active snapshot cannot resurrect a completed response', async f => {
      await f.stopPolling();
      await f.startStream('Final response remains visible.');
      const requestCount = f.snapshots().length;
      f.hold();
      await f.page.evaluate(() => { window.__pendingLoad = loadChatHistoryForSession(state.chatSessionId); });
      await waitForRequests(() => f.snapshots(), requestCount + 1);
      f.session.active = false;
      f.send({ type: 'output', content: '', done: true });
      f.send({ type: 'session_status', status: 'completed' });
      await f.page.waitForFunction(() => !chatSessionIsLive(state.chatSessionId));
      f.release();
      await f.page.evaluate(() => window.__pendingLoad);
      await settle(f.page);
      assert.equal(await f.page.evaluate(() => chatSessionIsLive(state.chatSessionId)), false);
      assert.equal(await f.page.locator('.chat-line-processing').count(), 0);
      assert.match(await f.page.locator('#chat-output').innerText(), /Final response remains visible\./);
    });

    await scenario('streaming retains manual section state and the selected text nodes', async f => {
      await f.stopPolling();
      await f.startStream('Stable selectable paragraph.\n\n### Thinking\n\n'
        + Array.from({ length: 12 }, (_, index) => `Thought paragraph ${index}.`).join('\n\n')
        + '\n\n### Response\n\nVisible answer.');
      await f.page.locator('#chat-output details.thinking-section summary').click();
      await f.page.waitForFunction(() => !chatStream.text.querySelector('details.thinking-section').open);
      await f.page.evaluate(() => {
        const paragraph = [...chatStream.text.querySelectorAll('p')].find(node => node.textContent === 'Stable selectable paragraph.');
        if (!paragraph) throw new Error('Stable paragraph missing');
        const text = paragraph.firstChild;
        const range = document.createRange();
        range.setStart(text, 0);
        range.setEnd(text, 'Stable selectable'.length);
        const selection = getSelection();
        selection.removeAllRanges();
        selection.addRange(range);
        window.__selectedTextNode = text;
        window.__manualDetails = chatStream.text.querySelector('details.thinking-section');
      });
      f.send({ type: 'output', content: '\n\nA further paragraph arrives.', done: false });
      await f.page.waitForFunction(() => state.chatBuffer.includes('A further paragraph arrives.'));
      await settle(f.page);
      assert.deepEqual(await f.page.evaluate(() => ({
        selection: getSelection().toString(),
        sameSelectionNode: getSelection().anchorNode === window.__selectedTextNode,
        sameDetails: chatStream.text.querySelector('details.thinking-section') === window.__manualDetails,
        open: chatStream.text.querySelector('details.thinking-section').open,
      })), { selection: 'Stable selectable', sameSelectionNode: true, sameDetails: true, open: false });
    });

    await scenario('auto-follow pauses on wheel-up and resumes only on returning to latest', async f => {
      await f.stopPolling();
      await f.startStream('Initial followed output.');
      const gap = () => f.page.evaluate(() => {
        const output = document.querySelector('#chat-output');
        return output.scrollHeight - output.clientHeight - output.scrollTop;
      });
      assert.ok(await gap() <= 1, 'initial auto-follow reaches bottom');
      const bounds = await f.page.locator('#chat-output').boundingBox();
      await f.page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
      await f.page.mouse.wheel(0, -800);
      await f.page.waitForFunction(() => {
        const output = document.querySelector('#chat-output');
        return output.scrollHeight - output.clientHeight - output.scrollTop > 400;
      });
      await rememberReadingPosition(f.page);
      f.send({ type: 'output', content: '\n\nWhile reading older messages.\n\n'.repeat(8), done: false });
      await f.page.waitForFunction(() => state.chatBuffer.includes('While reading older messages.'));
      assertStableReading(await readingPosition(f.page), 'stream while paused');
      assert.ok(await gap() > 400, 'new chunks do not pull the reader to bottom');
      await f.page.locator('#chat-jump-latest').click();
      await settle(f.page);
      assert.ok(await gap() <= 1, 'explicit latest resumes following');
      f.send({ type: 'output', content: '\n\nAfter explicitly following.\n\n'.repeat(6), done: false });
      await f.page.waitForFunction(() => state.chatBuffer.includes('After explicitly following.'));
      await settle(f.page);
      assert.ok(await gap() <= 1, 'following remains at bottom on later chunks');
      await f.page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
      await f.page.mouse.wheel(0, -800);
      await f.page.waitForFunction(() => {
        const output = document.querySelector('#chat-output');
        return output.scrollHeight - output.clientHeight - output.scrollTop > 400;
      });
      await f.page.mouse.wheel(0, 100000);
      await f.page.waitForFunction(() => {
        const output = document.querySelector('#chat-output');
        return output.scrollHeight - output.clientHeight - output.scrollTop <= 1;
      });
      f.send({ type: 'output', content: '\n\nAfter scrolling back to bottom.\n\n'.repeat(6), done: false });
      await f.page.waitForFunction(() => state.chatBuffer.includes('After scrolling back to bottom.'));
      await settle(f.page);
      assert.ok(await gap() <= 1, 'scrolling back to bottom also resumes following');
    }, { autoScroll: true });

    await scenario('concurrent older/tail loads stay valid and tail loads are single-flight', async f => {
      await f.stopPolling();
      f.hold('older');
      f.hold();
      const initialSnapshots = f.snapshots().length;
      const initialOlder = f.olderRequests().length;
      await f.page.evaluate(() => {
        window.__olderLoads = Promise.all([
          loadChatHistoryForSession(state.chatSessionId, { older: true }),
          loadChatHistoryForSession(state.chatSessionId, { older: true }),
        ]);
      });
      await waitForRequests(() => f.olderRequests(), initialOlder + 1);
      await f.page.evaluate(() => {
        window.__tailLoads = Promise.all(Array.from({ length: 3 }, () => loadChatHistoryForSession(state.chatSessionId)));
      });
      await settle(f.page);
      // Serializing older and tail requests is valid; neither caller may be
      // invalidated merely because the other operation was requested.
      assert.ok(f.snapshots().length <= initialSnapshots + 1, 'at most one in-flight tail request');
      assert.equal(f.olderRequests().length, initialOlder + 1, 'only one in-flight older request');
      f.release('older');
      assert.deepEqual(await f.page.evaluate(() => window.__olderLoads), [true, true]);
      await waitForRequests(() => f.snapshots(), initialSnapshots + 1);
      assert.equal(f.snapshots().length, initialSnapshots + 1, 'shared tail request starts exactly once');
      f.release();
      assert.deepEqual(await f.page.evaluate(() => window.__tailLoads), [true, true, true]);
      assert.deepEqual(await f.page.evaluate(() => chatHistoryWindow.messages.map(message => message.id)),
        Array.from({ length: 60 }, (_, index) => `m${index}`));
    });

    await scenario('older history cannot erase a response completed and a prompt submitted during its request', async f => {
      await f.stopPolling();
      await f.startStream('Completed while an older page was loading.');
      f.hold('older');
      const initialOlder = f.olderRequests().length;
      await f.page.evaluate(() => {
        window.__pendingOlder = loadChatHistoryForSession(state.chatSessionId, { older: true });
      });
      await waitForRequests(() => f.olderRequests(), initialOlder + 1);
      f.session.active = false;
      f.send({ type: 'output', content: '', done: true });
      f.send({ type: 'session_status', status: 'completed' });
      await f.page.waitForFunction(() => !chatSessionIsLive(state.chatSessionId));
      f.session.active = true;
      await f.page.locator('#chat-prompt').fill('A new prompt while the older page is still pending.');
      await f.page.locator('#chat-form').evaluate(form => form.requestSubmit());
      await f.page.waitForFunction(() => chatHistoryWindow.messages.some(message => message.content === 'A new prompt while the older page is still pending.'));
      const lateIds = await f.page.evaluate(() => {
        const ids = chatHistoryWindow.messages.filter(message => message.id.startsWith('local-')).map(message => message.id);
        window.__lateRows = ids.map(id => document.querySelector(`[data-chat-history-key="${id}"]`));
        return ids;
      });
      assert.equal(lateIds.length, 2, 'completed assistant and new prompt are both present');
      f.release('older');
      assert.equal(await f.page.evaluate(() => window.__pendingOlder), true);
      assert.deepEqual(await f.page.evaluate(() => chatHistoryWindow.messages.map(message => message.id)),
        [...Array.from({ length: 60 }, (_, index) => `m${index}`), ...lateIds]);
      assert.deepEqual(await f.page.evaluate(() => [...document.querySelectorAll('[data-chat-history-key]')].map(node => node.dataset.chatHistoryKey)),
        [...Array.from({ length: 60 }, (_, index) => `m${index}`), ...lateIds]);
      assert.equal(await f.page.evaluate(() => window.__lateRows.every(node => node?.isConnected)), true);
    });
  }
});
