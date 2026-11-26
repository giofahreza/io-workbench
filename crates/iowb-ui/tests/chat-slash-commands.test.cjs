const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { test } = require('node:test');
const vm = require('node:vm');

function fixture() {
  const context = vm.createContext({ console, URLSearchParams, Event });
  vm.runInContext(readFileSync(join(__dirname, '../static/app/chat/slash-commands.js'), 'utf8'), context);
  vm.runInContext(`
    const state = { chatSessionId: 'session-1', chatImages: [] };
    const input = { value: '', focus() {}, setAttribute() {}, dispatchEvent() {} };
    const menu = { hidden: true, replaceChildren() {}, append() {} };
    const document = { createElement: () => ({ setAttribute() {}, addEventListener() {} }) };
    const window = { alert: (message) => actions.push('alert:' + message) };
    const setChatPromptValue = (value) => { input.value = value; };
    const requests = [], errors = [], actions = [];
    let project = '/project', model = 'sonnet', provider = 'claude';
    let knownSession = null;
    const findChatSession = () => knownSession;
    const qs = (selector) => selector === '#chat-prompt' ? input
      : selector === '#chat-slash-commands' ? menu : null;
    const chatCliValue = () => provider;
    const chatModelValue = () => model;
    const activeProjectPath = () => project;
    const stagedChatEdit = () => null;
    const api = (url, options) => new Promise((resolve, reject) => requests.push({ url, options, resolve, reject }));
    const showError = (error) => errors.push(error.message);
    const clearChatPromptInput = () => { input.value = ''; actions.push('clear'); };
    const showToast = (message) => actions.push(message);
    const selectedRunningChatSession = () => false;
    const togglePromptConfigPanel = (open) => actions.push('models:' + open);
    const startNewChatForProject = async () => { actions.push('new'); state.chatSessionId = 'local-new'; };
    const compactChatSessionContext = async () => actions.push('compact');
  `, context);
  return (source) => vm.runInContext(source, context);
}

test('slash recognition excludes paths, inline text, escaped names and oversized tokens', () => {
  const run = fixture();
  assert.equal(run(`chatSlashName(" /plugin:check 'a b'")`), 'plugin:check');
  for (const value of ['/', '/tmp/project', '/file.md', 'explain /help', '\\/help', '/' + 'a'.repeat(161)]) {
    assert.equal(run(`chatSlashName(${JSON.stringify(value)})`), null, value);
  }
});

test('persisted local-prefixed sessions retain their native command context', () => {
  const run = fixture();
  run(`state.chatSessionId = 'local-fixture'`);
  assert.equal(run('chatSlashContext().sessionId'), null);
  run(`knownSession = { id: 'local-fixture', messageCount: 2 }`);
  assert.equal(run('chatSlashContext().sessionId'), 'local-fixture');
});

test('Codex compaction accepts persisted local-prefixed chats, not unsent drafts', () => {
  const context = vm.createContext({ console });
  vm.runInContext(readFileSync(join(__dirname, '../static/app/chat/recovery.js'), 'utf8'), context);
  const run = (source) => vm.runInContext(source, context);
  run(`
    const state = { chatSessionId: 'local-fixture', chatRecoveryBySession: {}, chatProcessing: false };
    let session = null;
    const findChatSession = () => session;
    const cachedChatSession = () => null;
    const sessionProvider = (session) => session.provider;
    const chatCliValue = () => 'codex';
    const canLoadProtectedData = () => true;
    const selectedRunningChatSession = () => false;
    selectedChatIsStopping = () => false;
    const chatHistoryWindow = { sessionId: 'local-fixture', messages: [] };
  `);
  assert.equal(run('selectedChatCanManualCompact()'), false);
  run(`session = { id: 'local-fixture', provider: 'codex', messageCount: 2 }`);
  assert.equal(run('selectedChatCanManualCompact()'), true);
});

test('catalog requests coalesce and late results cannot overwrite a different project', async () => {
  const run = fixture();
  const first = run('loadChatSlashCatalog()');
  const repeated = run('loadChatSlashCatalog()');
  assert.equal(run('requests.length'), 1);
  run(`project = '/other'`);
  const next = run('loadChatSlashCatalog()');
  assert.equal(run('requests.length'), 2);
  run(`requests[1].resolve({ commands: [], revision: 'current' })`);
  await next;
  run(`requests[0].resolve({ commands: [], revision: 'stale' })`);
  await Promise.all([first, repeated]);
  assert.equal(run('chatSlashState.catalog.revision'), 'current');
});

test('unsupported commands preserve the draft and never fall through to a model turn', async () => {
  const run = fixture();
  run(`input.value = '/unknown-fixture'`);
  const pending = run(`interceptChatSlashCommand(input.value)`);
  run(`requests[0].reject(new Error('Unknown command'))`);
  assert.equal(await pending, true);
  assert.equal(run('input.value'), '/unknown-fixture');
  assert.equal(run('actions.length'), 0);
  assert.equal(run('errors[0]'), 'Unknown command');
});

test('late preparation and duplicate submits cannot clear a newer draft', async () => {
  const run = fixture();
  run(`input.value = '/status'`);
  const pending = run(`interceptChatSlashCommand(input.value)`);
  assert.equal(await run(`interceptChatSlashCommand(input.value)`), true);
  assert.equal(run('requests.length'), 1);
  run(`input.value = 'new draft'; requests[0].resolve({ action: 'status' })`);
  assert.equal(await pending, true);
  assert.equal(run('input.value'), 'new draft');
  assert.equal(run('actions.length'), 0);
});

test('late help discovery does not clear a draft edited after preparation', async () => {
  const run = fixture();
  run(`input.value = '/help'`);
  const pending = run('interceptChatSlashCommand(input.value)');
  run(`requests[0].resolve({ action: 'help' })`);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(run('requests.length'), 2);
  run(`input.value = 'new draft'; requests[1].resolve({ commands: [] })`);
  assert.equal(await pending, true);
  assert.equal(run('input.value'), 'new draft');
  assert.equal(run('actions.length'), 0);
});

test('validated native prompts fall through unchanged; model actions reveal configuration', async () => {
  const run = fixture();
  run(`input.value = '/plugin:check file'`);
  const prompt = run(`interceptChatSlashCommand(input.value)`);
  run(`requests[0].resolve({ action: 'prompt' })`);
  assert.equal(await prompt, false);
  assert.equal(run('input.value'), '/plugin:check file');
  run(`input.value = '/model'`);
  const model = run(`interceptChatSlashCommand(input.value)`);
  run(`requests[1].resolve({ action: 'model' })`);
  assert.equal(await model, true);
  assert.equal(run(`actions.includes('models:true')`), true);
  assert.equal(run('input.value'), '');
});

test('keyboard selection handles arrows, Tab, Enter and Escape, and yields when nothing matches', () => {
  const run = fixture();
  run(`
    chatSlashState.scope = JSON.stringify(chatSlashContext());
    chatSlashState.catalog = { commands: [
      { name: 'help', description: 'Help', available: true, source: 'workbench', verified: true },
      { name: 'history', description: 'History', available: true, source: 'workbench', verified: true },
    ] };
    input.value = '/h';
    menu.hidden = false;
    const key = (name) => {
      const event = { key: name, isComposing: false, shiftKey: false, prevented: false, preventDefault() { this.prevented = true; } };
      return JSON.stringify({ handled: handleChatSlashKeydown(event), prevented: event.prevented });
    };
  `);
  assert.equal(run(`key('ArrowDown')`), '{"handled":true,"prevented":true}');
  assert.equal(run('chatSlashState.selected'), 1);
  assert.equal(run(`key('ArrowUp')`), '{"handled":true,"prevented":true}');
  assert.equal(run('chatSlashState.selected'), 0);
  assert.equal(run(`key('Tab')`), '{"handled":true,"prevented":true}');
  assert.equal(run('input.value'), '/help ');
  run(`input.value = '/h'; chatSlashState.selected = 1; menu.hidden = false`);
  assert.equal(run(`key('Enter')`), '{"handled":true,"prevented":true}');
  assert.equal(run('input.value'), '/history ');
  run(`input.value = '/h'; menu.hidden = false`);
  assert.equal(run(`key('Escape')`), '{"handled":true,"prevented":true}');
  assert.equal(run('chatSlashState.hiddenFor'), '/h');
  assert.equal(run('menu.hidden'), true);
  assert.equal(run(`key('Enter')`), '{"handled":false,"prevented":false}');
  run(`input.value = '/zzz'; chatSlashState.hiddenFor = null; menu.hidden = false`);
  assert.equal(run(`key('Enter')`), '{"handled":false,"prevented":false}');
});

test('attachments block real commands but not slash prose', async () => {
  const run = fixture();
  run(`state.chatImages = [{}]; input.value = '/tmp is full'`);
  const prose = run('interceptChatSlashCommand(input.value)');
  run(`requests[0].resolve({ action: 'prompt', prompt: input.value, command: null })`);
  assert.equal(await prose, false);
  assert.equal(run('errors.length'), 0);
  run(`input.value = '/init'`);
  const command = run('interceptChatSlashCommand(input.value)');
  run(`requests[1].resolve({ action: 'prompt', prompt: '/init', command: { name: 'init' } })`);
  assert.equal(await command, true);
  assert.equal(run('errors[0]'), 'Send commands separately from attachments or edit-from-here.');
  assert.equal(run('input.value'), '/init');
});

test('mcp shows the server message and clears the draft', async () => {
  const run = fixture();
  run(`input.value = '/mcp'`);
  const pending = run('interceptChatSlashCommand(input.value)');
  run(`requests[0].resolve({ action: 'mcp', message: 'github — connected', command: { name: 'mcp' } })`);
  assert.equal(await pending, true);
  assert.equal(run(`actions.includes('alert:github — connected')`), true);
  assert.equal(run('input.value'), '');
});

test('new and clear switch to a local draft without deleting server history', async () => {
  const run = fixture();
  run(`input.value = '/clear'`);
  const pending = run('interceptChatSlashCommand(input.value)');
  run(`requests[0].resolve({ action: 'new' })`);
  assert.equal(await pending, true);
  assert.equal(run('state.chatSessionId'), 'local-new');
  assert.equal(run('requests.length'), 1);
  assert.equal(run(`requests[0].url`), '/api/chat/commands/prepare');
});
