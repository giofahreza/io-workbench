const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { test } = require('node:test');

test('chat rendering preserves keyed rows, disclosures, focus, and selected text', {
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
      await page.setContent('<div id="chat-output"></div>');
      for (const file of ['vendor/markdown-it/markdown-it.min.js', 'app/chat/render.js', 'app/chat/history.js', 'app/chat/stream.js', 'app/chat/recovery.js', 'app/workspace/git/markdown.js']) {
        await page.addScriptTag({ content: readFileSync(join(__dirname, '../static', file), 'utf8') });
      }
      const result = await page.evaluate(() => {
        window.state = { chatSessionId: 'session', preferences: {} };
        window.chatDisplaySettings = () => ({ expandThinking: true, expandParameters: true, autoScrollToBottom: false });
        window.qs = (selector) => document.querySelector(selector);
        window.firstDefined = (...values) => values.find((value) => value !== undefined && value !== null);
        window.escapeHtml = (value) => String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;').replaceAll("'", '&#039;');
        window.normalizeMessageMeta = (meta) => meta;
        window.getSessionOverridesFor = () => ({});
        window.syncChatPromptHistoryFromMessages = () => {};
        window.findChatSession = () => ({ id: 'session', provider: 'claude' });
        window.renderChatBubbleHtml = (text) => renderMarkdownLiteWithSections(text, { assistant: true }).body;
        window.sidebarProviderIcon = () => 'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==';
        window.sidebarProviderLabel = (provider) => provider;
        window.selectedRunningChatSession = () => null;
        window.scrollChatToBottom = () => {};
        const messages = [
          { id: 'user', role: 'user', content: 'A question' },
          { id: 'assistant', role: 'assistant', content: 'thinking\nReasoning stays closed\n\nassistant\n[Reference](https://example.com)\n\nKeep this paragraph.' },
          { id: 'tool', role: 'tool', content: 'exec / Details\nThe output' },
        ];
        replayChatMessages(messages);
        const output = chatOutputRoot();
        const rows = Array.from(output.children);
        const live = document.createElement('div');
        live.textContent = 'live';
        output.append(live);
        const body = rows[1].querySelector('.chat-line-text');
        const thinking = body.querySelector('details');
        thinking.open = false;
        const toolDetails = rows[2].querySelector('details');
        toolDetails.open = true;
        const link = body.querySelector('a');
        link.focus();
        const footerButton = rows[1].querySelector('[data-chat-line-footer-details]');
        const paragraph = Array.from(body.querySelectorAll('p')).find((node) => node.textContent === 'Keep this paragraph.');
        const selectedText = paragraph.firstChild;
        window.getSelection().setBaseAndExtent(selectedText, 0, selectedText, 9);
        const mutations = [];
        const observer = new MutationObserver(() => {});
        observer.observe(output, { childList: true });
        for (let index = 0; index < 5; index += 1) reconcileChatMessages(messages);
        mutations.push(...observer.takeRecords());
        const unchanged = {
          rows: rows.every((node, index) => output.children[index] === node),
          noMoves: mutations.length === 0,
          footer: rows[1].querySelector('[data-chat-line-footer-details]') === footerButton,
          focus: document.activeElement === link,
          selection: window.getSelection().toString(),
        };
        reconcileChatMessages(messages.map((message) => message.id === 'assistant'
          ? { ...message, content: `${message.content} Still growing.` }
          : message.id === 'tool' ? { ...message, content: `${message.content}\nMore output` } : message));
        const updated = {
          paragraph: body.contains(paragraph) && paragraph.firstChild === selectedText,
          thinking: body.querySelector('details') === thinking && !thinking.open,
          tool: rows[2].querySelector('details') === toolDetails && toolDetails.open,
          focus: document.activeElement === link,
          selection: window.getSelection().toString(),
          live: output.lastElementChild === live,
        };
        const saved = document.createElement('div');
        renderChatMessageContent(saved, 'Stable text');
        output.append(saved);
        window.getSelection().setBaseAndExtent(saved.querySelector('p').firstChild, 0, saved.querySelector('p').firstChild, 6);
        renderChatMessageContent(saved, '**Stable** text');
        const structuralSelection = window.getSelection().toString();
        renderChatMessageContent(saved, '<img src=x onerror="window.UNSAFE=true">\n\n[unsafe](javascript:alert(1))');
        const safe = !saved.querySelector('img,script,[onerror],[href^="javascript:"]');
        reconcileChatMessages(messages.slice(0, 2));
        observer.disconnect();
        window.isActiveChatSessionEvent = ({ sessionId }) => sessionId === 'session';
        window.updateChatComposerState = () => {};
        let retryCount = 0;
        window.compactChatSessionContext = async () => { retryCount += 1; };
        state.chatRecoveryBySession = { session: { state: 'required', message: 'Needs recovery', observedBytes: 1000, limitBytes: 500 } };
        state.chatManualCompactionBySession = { session: { state: 'failed', message: 'Retry compact' } };
        renderChatRecoveryCard('session');
        renderChatManualCompactionCard('session');
        const recovery = output.querySelector('.chat-context-recovery');
        const recoveryButton = recovery.querySelector('button');
        const compact = output.querySelector('.chat-manual-compaction');
        const retry = compact.querySelector('[data-chat-manual-compact-retry]');
        retry.focus();
        observer.observe(output, { childList: true, subtree: true, attributes: true, characterData: true });
        for (let index = 0; index < 5; index += 1) {
          renderChatRecoveryCard('session');
          renderChatManualCompactionCard('session');
        }
        const cardMutations = observer.takeRecords().length;
        state.chatRecoveryBySession.session = { ...state.chatRecoveryBySession.session, message: 'Different recovery message' };
        renderChatRecoveryCard('session');
        state.chatManualCompactionBySession.session = { state: 'starting' };
        renderChatManualCompactionCard('session');
        const sameBusyRetry = compact.querySelector('[data-chat-manual-compact-retry]') === retry && retry.disabled;
        state.chatManualCompactionBySession.session = { state: 'failed', message: 'Retry again' };
        renderChatManualCompactionCard('session');
        const sameRetry = compact.querySelector('[data-chat-manual-compact-retry]') === retry && !retry.disabled;
        retry.click();
        compact.querySelector('[data-chat-manual-compact-dismiss]').click();
        observer.disconnect();
        return {
          unchanged, updated, structuralSelection, safe, removedMissing: !rows[2].isConnected, liveRetained: live.isConnected,
          cards: {
            cardMutations, sameBusyRetry, sameRetry, retryCount,
            recoveryRetained: output.querySelector('.chat-context-recovery') === recovery && recovery.querySelector('button') === recoveryButton,
            compactDismissed: !compact.isConnected,
          },
        };
      });
      assert.deepEqual(result.unchanged, { rows: true, noMoves: true, footer: true, focus: true, selection: 'Keep this' });
      assert.deepEqual(result.updated, { paragraph: true, thinking: true, tool: true, focus: true, selection: 'Keep this', live: true });
      assert.equal(result.structuralSelection, 'Stable');
      assert.equal(result.safe, true);
      assert.equal(result.removedMissing, true);
      assert.equal(result.liveRetained, true);
      assert.deepEqual(result.cards, { cardMutations: 0, sameBusyRetry: true, sameRetry: true, retryCount: 1, recoveryRetained: true, compactDismissed: true });
      await page.close();
    }
  } finally {
    await browser.close();
  }
});
