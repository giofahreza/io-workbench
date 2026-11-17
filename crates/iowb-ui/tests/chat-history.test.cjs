const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { test } = require('node:test');
const vm = require('node:vm');

function fixture() {
  const context = vm.createContext({ console });
  for (const file of ['history.js', 'stream.js']) {
    vm.runInContext(readFileSync(join(__dirname, '../static/app/chat', file), 'utf8'), context);
  }
  vm.runInContext(`
    const state = {
      chatSessionId: 'session', chatOlderMessagesLoadingSessionId: '',
      chatRecoveryBySession: {}, chatOutputBuffersBySession: {},
    };
    const CHAT_HISTORY_LOAD_THRESHOLD_PX = 72;
    const output = { scrollTop: 0, scrollHeight: 100, children: [] };
    const requests = [];
    const errors = [];
    let prepends = 0;
    let resets = 0;
    chatHistoryWindow = { sessionId: 'session', offset: 300, totalCount: 330,
      messages: [{ id: 'existing', role: 'user', content: 'existing' }] };
    chatOutputRoot = () => output;
    activeView = () => 'chat';
    stagedChatEdit = () => null;
    api = (url) => new Promise((resolve, reject) => requests.push({ url, resolve, reject }));
    findChatSession = () => null;
    isBoardChatSession = () => false;
    splitCachedStreamingMessage = (messages) => ({ messages });
    sessionProvider = () => 'codex';
    captureChatHistoryAnchor = () => null;
    restoreChatHistoryAnchor = () => false;
    captureChatViewport = () => null;
    getSessionOverridesFor = () => ({});
    prependChatHistoryPage = () => { prepends += 1; };
    resetChatOutputDom = () => { resets += 1; };
    renderOlderChatMessagesControl = () => {};
    loadSessionOverridesIntoState = () => {};
    renderChatFooter = () => {};
    rememberCurrentChatSession = () => {};
    persistActiveChatSelection = () => {};
    activeProjectPath = () => '';
    updateChatEmptyState = () => {};
    showError = (error) => errors.push(error.message);
  `, context);
  return (code) => vm.runInContext(code, context);
}

test('filtered pages cannot drain history through layout or completion callbacks', async () => {
  const run = fixture();
  run('maybeLoadOlderChatMessages()');
  assert.equal(run('requests.length'), 0);
  run('requestOlderChatMessagesDemand()');
  assert.equal(run('requests.length'), 1);
  assert.match(run('requests[0].url'), /limit=30&offset=270$/);
  const pending = run('chatOlderHistoryRequest.promise');
  run('requestOlderChatMessagesDemand(); maybeLoadOlderChatMessages()');
  run('requests[0].resolve({ messages: [], totalCount: 330 })');
  await pending;
  run('maybeLoadOlderChatMessages(); maybeLoadOlderChatMessages()');
  assert.equal(run('requests.length'), 1);
  assert.equal(run('chatHistoryWindow.offset'), 270);
  assert.equal(run('resets'), 0);
  assert.equal(run('prepends'), 1);
  assert.equal(run('errors.length'), 0);
  run('requestOlderChatMessagesDemand()');
  assert.equal(run('requests.length'), 2);
});

test('concurrent older loads share a request and deduplicate overlapping IDs', async () => {
  const run = fixture();
  const first = run("loadChatHistoryForSession('session', { older: true })");
  const second = run("loadChatHistoryForSession('session', { older: true })");
  assert.equal(run('requests.length'), 1);
  run(`requests[0].resolve({ messages: [
    { id: 'older', role: 'user', content: 'older' },
    { id: 'existing', role: 'user', content: 'existing' },
  ], totalCount: 330 })`);
  assert.equal(await first, true);
  assert.equal(await second, true);
  assert.equal(run('chatHistoryWindow.messages.length'), 2);
  assert.equal(run('state.chatOlderMessagesLoadingSessionId'), '');
});

test('stale completion cannot overwrite a switched session', async () => {
  const run = fixture();
  const pending = run("loadChatHistoryForSession('session', { older: true })");
  run(`state.chatSessionId = 'other'; chatHistoryGeneration += 1;
    chatHistoryWindow = { sessionId: 'other', offset: 30, messages: [] };
    requests[0].resolve({ messages: [{ id: 'stale' }], totalCount: 330 });`);
  assert.equal(await pending, false);
  assert.equal(run('chatHistoryWindow.sessionId'), 'other');
  assert.equal(run('prepends'), 0);
});

test('failure releases the gate and needs fresh user demand to retry', async () => {
  const run = fixture();
  const pending = run("loadChatHistoryForSession('session', { older: true })");
  run("requests[0].reject(new Error('network unavailable'))");
  assert.equal(await pending, false);
  assert.equal(run('state.chatOlderMessagesLoadingSessionId'), '');
  run('maybeLoadOlderChatMessages()');
  assert.equal(run('requests.length'), 1);
  run('requestOlderChatMessagesDemand()');
  assert.equal(run('requests.length'), 2);
});

function tailFixture(count = 60, offset = 0) {
  const run = fixture();
  run(`
    const rows = Array.from({ length: ${count} }, (_, i) => ({
      id: 'm' + i, role: 'user', content: 'message ' + i,
    }));
    chatHistoryWindow = { sessionId: 'session', offset: ${offset},
      totalCount: rows.length, persistedTotalCount: rows.length, messages: rows.slice(${offset}) };
  `);
  return run;
}

test('an overlapping tail preserves every loaded older row without another request', async () => {
  const run = tailFixture();
  const before = run('chatHistoryWindow.messages[0]');
  const result = await run(`chatHistoryTailWindow('session',
    rows.slice(34).concat(Array.from({ length: 4 }, (_, i) => ({ id: 'm' + (60 + i) }))), 64, chatHistoryGeneration)`);
  assert.equal(result.messages.length, 64);
  assert.equal(result.messages[0], before);
  assert.equal(result.offset, 0);
  assert.equal(run('requests.length'), 0);
});

test('a disjoint tail fills the gap in bounded pages before replacing the model', async () => {
  const run = tailFixture(60, 30);
  run("rows.push(...Array.from({ length: 65 }, (_, i) => ({ id: 'm' + (60 + i) })))");
  const pending = run("chatHistoryTailWindow('session', rows.slice(-30), rows.length, chatHistoryGeneration)");
  for (const [index, offset, limit] of [[0, 30, 30], [1, 60, 30], [2, 90, 5]]) {
    assert.match(run(`requests[${index}].url`), new RegExp(`limit=${limit}&offset=${offset}$`));
    run(`requests[${index}].resolve({ messages: rows.slice(${offset}, ${offset + limit}), totalCount: rows.length })`);
    await Promise.resolve();
  }
  const result = await pending;
  assert.equal(result.offset, 30);
  assert.deepEqual(Array.from(result.messages, row => row.id), Array.from({ length: 95 }, (_, i) => `m${30 + i}`));
});

test('deletion and shifted boundaries refetch the covered range without retaining deleted rows', async () => {
  const run = tailFixture();
  run('rows.splice(20, 1)');
  const pending = run("chatHistoryTailWindow('session', rows.slice(-30), rows.length, chatHistoryGeneration)");
  assert.match(run('requests[0].url'), /limit=29&offset=0$/);
  run('requests[0].resolve({ messages: rows.slice(0, 29), totalCount: rows.length })');
  const result = await pending;
  assert.equal(result.messages.length, 59);
  assert.equal(result.messages.some(row => row.id === 'm20'), false);
});

test('cross-page count drift or session switching cannot commit an incoherent range', async () => {
  for (const change of ['count', 'session']) {
    const run = tailFixture();
    run("rows.push(...Array.from({ length: 40 }, (_, i) => ({ id: 'm' + (60 + i) })))");
    const pending = run("chatHistoryTailWindow('session', rows.slice(-30), rows.length, chatHistoryGeneration)");
    if (change === 'session') run("state.chatSessionId = 'other'; chatHistoryGeneration += 1");
    run(`requests[0].resolve({ messages: rows.slice(0, 30), totalCount: ${change === 'count' ? 101 : 100} })`);
    assert.equal(await pending, null);
    assert.equal(run('chatHistoryWindow.messages.length'), 60);
  }
});

test('bounded projection does not shorten an already loaded complete body', () => {
  const run = tailFixture();
  const richer = run(`retainRicherChatMessage(
    { id: 'm', content: 'complete message', metadata: {} },
    { id: 'm', content: 'complete\\n[truncated]', metadata: { contentTruncated: true, originalContentBytes: 16 } })`);
  assert.equal(richer.content, 'complete message');
  assert.equal(richer.metadata.contentTruncated, false);
  const changed = run(`retainRicherChatMessage(
    { id: 'm', content: 'complete message', metadata: {} },
    { id: 'm', content: 'changed\\n[truncated]', metadata: { contentTruncated: true, originalContentBytes: 16 } })`);
  assert.equal(changed.content, 'changed\n[truncated]');
});
