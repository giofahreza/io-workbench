const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { extname, resolve, sep } = require('node:path');

const ORIGIN = 'http://chat.test';
const STATIC_ROOT = resolve(__dirname, '../../static');
const MIME_TYPES = {
  '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css',
  '.svg': 'image/svg+xml', '.png': 'image/png', '.webp': 'image/webp',
  '.ico': 'image/x-icon', '.json': 'application/json', '.woff2': 'font/woff2',
};

function syntheticMessage(index, sessionId = 'scroll-regression') {
  return {
    id: `m${index}`, sessionId, role: index % 2 ? 'assistant' : 'user',
    content: `Synthetic message ${index}\n\n${Array.from({ length: 6 }, (_, paragraph) =>
      `Paragraph ${paragraph} for synthetic message ${index}.`).join('\n\n')}`,
    timestamp: new Date(Date.UTC(2026, 8, 30, 0, index)).toISOString(),
  };
}

// Real application startup, DOM, CSS, API client, websocket handlers and timers.
// Only external I/O is synthetic. Nothing is read from a running workbench or a
// developer's config, and unexpected external resources are blocked.
async function openChatScrollPage(browser, { width = 1280, autoScroll = false } = {}) {
  const context = await browser.newContext({
    viewport: { width, height: 900 }, serviceWorkers: 'block',
  });
  const page = await context.newPage();
  page.setDefaultTimeout(12000);
  const errors = [];
  const requests = [];
  const sockets = [];
  const clientMessages = [];
  const gates = new Map();
  const session = {
    id: 'scroll-regression', provider: 'claude', title: 'Synthetic scroll regression',
    projectPath: '/synthetic/project', messageCount: 60, active: false,
    updatedAt: '2026-09-30T01:00:00Z',
  };
  const messages = Array.from({ length: 60 }, (_, index) => syntheticMessage(index, session.id));
  const conversations = new Map([[session.id, { session, messages }]]);
  let sequence = 0;

  page.on('pageerror', error => errors.push(error.message));
  await page.routeWebSocket('**/ws**', socket => {
    sockets.push(socket);
    socket.onMessage(data => clientMessages.push(JSON.parse(String(data))));
  });
  await page.route('**/*', async route => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.origin !== ORIGIN) {
      errors.push(`Unexpected external request: ${url.origin}${url.pathname}`);
      return route.abort();
    }
    if (!url.pathname.startsWith('/api/') && url.pathname !== '/health') {
      if (url.pathname === '/test/delayed-image.svg') {
        if (gates.has('image')) await gates.get('image').promise;
        return route.fulfill({
          status: 200, contentType: 'image/svg+xml',
          body: '<svg xmlns="http://www.w3.org/2000/svg" width="500" height="440"><rect width="500" height="440" fill="steelblue"/></svg>',
        });
      }
      const file = resolve(STATIC_ROOT, `.${url.pathname === '/' ? '/index.html' : url.pathname}`);
      if (!file.startsWith(`${STATIC_ROOT}${sep}`)) return route.abort();
      try {
        return await route.fulfill({
          status: 200, contentType: MIME_TYPES[extname(file)] || 'application/octet-stream',
          body: readFileSync(file),
        });
      } catch (error) {
        errors.push(`Static resource unavailable: ${url.pathname}: ${error.message}`);
        return route.fulfill({ status: 404, body: '' });
      }
    }
    const entry = {
      path: url.pathname, query: url.search, method: request.method(), startedAt: Date.now(),
    };
    requests.push(entry);
    let body = {};
    let gateKind = '';
    const requestSessionId = decodeURIComponent(url.pathname.match(/\/sessions\/([^/]+)\/(?:snapshot|messages)$/)?.[1] || session.id);
    const conversation = conversations.get(requestSessionId) || { session, messages };
    if (url.pathname === '/health') body = { status: 'ok', service: 'synthetic', version: 'test' };
    else if (url.pathname === '/api/auth/status') body = { enabled: false, isAuthenticated: true, needsSetup: false };
    else if (url.pathname === '/api/projects') body = { projects: [{ name: 'Synthetic project', path: session.projectPath,
      sessions: [...conversations.values()].map(value => ({ ...value.session })) }] };
    else if (url.pathname.endsWith('/snapshot')) {
      const limit = Number(url.searchParams.get('limit')) || 30;
      body = { session: { ...conversation.session }, messages: conversation.messages.slice(-limit), totalCount: conversation.messages.length,
        hasMore: conversation.messages.length > limit, recovery: null };
      if (limit !== 1) gateKind = 'snapshot';
    } else if (url.pathname.endsWith('/messages')) {
      const offset = Number(url.searchParams.get('offset')) || 0;
      const limit = Number(url.searchParams.get('limit')) || 30;
      body = { messages: conversation.messages.slice(offset, offset + limit), totalCount: conversation.messages.length, hasMore: offset > 0 };
      gateKind = 'older';
    } else if (url.pathname.endsWith('/draft')) body = { content: '' };
    else if (url.pathname.endsWith('/prompts')) body = { prompts: [], hasMore: false };
    else if (url.pathname.endsWith('/sessions')) body = { sessions: [...conversations.values()].map(value => ({ ...value.session })) };
    else if (url.pathname === '/api/settings') body = { settings: [] };
    else if (url.pathname === '/api/settings/sidebar-active-sessions') body = { pinnedSessions: [], initialized: true };
    // Serialize at request time, not release time, to model a legitimately stale
    // response while subsequent websocket events and database writes arrive.
    const serialized = JSON.stringify(body);
    const gate = gates.get(`${gateKind}:${requestSessionId}`) || gates.get(gateKind);
    if (gate) await gate.promise;
    entry.completedAt = Date.now();
    await route.fulfill({ status: 200, contentType: 'application/json', body: serialized });
  });
  await page.addInitScript(({ autoScroll }) => {
    localStorage.setItem('iowb.lastView', 'chat');
    localStorage.setItem('iowb.webPreferences', JSON.stringify({
      chatAutoScrollToBottom: autoScroll, chatExpandThinking: true, chatExpandParameters: true,
    }));
    window.addEventListener('iowb:app-ready', () => { window.__scrollTestAppReady = true; });
  }, { autoScroll });
  await page.goto(ORIGIN, { waitUntil: 'networkidle' });
  await page.waitForFunction(() => window.__scrollTestAppReady === true
    && state.chatSessionId === 'scroll-regression'
    && document.querySelectorAll('[data-chat-history-key]').length === 30);
  await page.waitForFunction(() => state.ws?.readyState === WebSocket.OPEN);
  await settle(page);

  const fixture = {
    page, context, errors, requests, session, messages, clientMessages,
    addSession(id, count = 6) {
      const otherSession = { ...session, id, title: `Synthetic ${id}`, messageCount: count, active: false };
      const otherMessages = Array.from({ length: count }, (_, index) => ({
        ...syntheticMessage(index, id), id: `${id}-m${index}`,
      }));
      const conversation = { session: otherSession, messages: otherMessages };
      conversations.set(id, conversation);
      return conversation;
    },
    send(payload) {
      assert.ok(sockets.length > 0, 'application websocket is connected');
      for (const socket of sockets) socket.send(JSON.stringify({
        provider: 'claude', sessionId: session.id,
        responseId: 'test-response', sequence: ++sequence, ...payload,
      }));
    },
    hold(kind = 'snapshot') {
      assert.equal(gates.has(kind), false, `only one ${kind} response gate`);
      let release;
      const promise = new Promise(resolve => { release = resolve; });
      gates.set(kind, { promise, release });
    },
    release(kind = 'snapshot') {
      gates.get(kind)?.release();
      gates.delete(kind);
    },
    snapshots() { return requests.filter(request => request.path.endsWith('/snapshot') && request.query !== '?limit=1'); },
    olderRequests() { return requests.filter(request => request.path.endsWith('/messages')); },
    async stopPolling() {
      await page.evaluate(() => {
        clearInterval(state.chatActivityPollTimer);
        state.chatActivityPollTimer = null;
      });
    },
    async startStream(content = 'Live response.') {
      session.active = true;
      fixture.send({ type: 'session_status', status: 'running' });
      fixture.send({ type: 'output', content, done: false });
      await page.waitForFunction(content => state.chatBuffer.includes(content), content);
      await settle(page);
    },
    async close() {
      for (const gate of gates.values()) gate.release();
      gates.clear();
      await context.close();
    },
  };
  assert.deepEqual(errors, [], 'fixture starts without runtime or resource errors');
  return fixture;
}

async function settle(page) {
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

async function waitForRequests(readRequests, count) {
  const deadline = Date.now() + 12000;
  while (readRequests().length < count) {
    if (Date.now() > deadline) throw new Error(`Expected ${count} requests; received ${readRequests().length}`);
    await new Promise(resolve => setTimeout(resolve, 20));
  }
}

async function rememberReadingPosition(page, scrollTop) {
  if (scrollTop !== undefined) {
    await page.evaluate(top => { document.querySelector('#chat-output').scrollTop = top; }, scrollTop);
    await settle(page);
  }
  return page.evaluate(() => {
    const output = document.querySelector('#chat-output');
    const top = output.getBoundingClientRect().top;
    const rows = [...output.querySelectorAll('[data-chat-history-key]')];
    const node = rows.find(row => row.getBoundingClientRect().bottom > top);
    if (!node) throw new Error('No visible persisted row to use as an anchor');
    window.__scrollTestReading = {
      node, offset: node.getBoundingClientRect().top - top,
      rows, control: output.querySelector('[data-chat-load-older]'),
    };
    return { top: output.scrollTop, key: node.dataset.chatHistoryKey, rows: rows.length };
  });
}

async function readingPosition(page) {
  await settle(page);
  return page.evaluate(() => {
    const output = document.querySelector('#chat-output');
    const before = window.__scrollTestReading;
    return {
      top: output.scrollTop,
      drift: before.node.getBoundingClientRect().top - output.getBoundingClientRect().top - before.offset,
      anchorConnected: before.node.isConnected,
      allRowsRetained: before.rows.every(row => row.isConnected && row.parentElement === output),
      controlRetained: before.control === output.querySelector('[data-chat-load-older]'),
      keys: [...output.querySelectorAll('[data-chat-history-key]')].map(node => node.dataset.chatHistoryKey),
    };
  });
}

module.exports = { openChatScrollPage, readingPosition, rememberReadingPosition, settle, syntheticMessage, waitForRequests };
