const assert = require('node:assert/strict');
const { test } = require('node:test');
const { openChatScrollPage, settle } = require('./fixtures/chat-scroll-page.cjs');

const enabled = !!process.env.PLAYWRIGHT_MODULE;
const shell = { command: '/usr/bin/zsh', args: [], source: 'environment', safe: false };
const chunk = (sequence, data) => ({ sequence, data, stream: 'stdout' });
const snapshot = (id, chunks = [], extra = {}) => ({
  process: { id, command: shell.command, args: [], pty: true }, chunks,
  firstSequence: chunks[0]?.sequence || 1, lastSequence: chunks.at(-1)?.sequence || 0,
  running: true, exitCode: null, error: null, ...extra,
});

async function terminalPage(browser, options = {}) {
  const fixture = await openChatScrollPage(browser);
  await fixture.stopPolling();
  fixture.starts = [];
  fixture.snapshots = [];
  fixture.deletes = [];
  fixture.current = snapshot('terminal-1', [chunk(1, 'INITIAL_PROMPT\r\n')]);
  await fixture.page.route('**/api/process/**', async route => {
    if (route.request().method() === 'DELETE') fixture.deletes.push(new URL(route.request().url()).pathname.split('/').at(-1));
    if (options.process) return options.process(route, fixture);
    await route.fulfill({ status: 200, contentType: 'application/json', body: '{}' });
  });
  await fixture.page.route('**/api/terminal**', async route => {
    if (route.request().method() === 'POST') {
      fixture.starts.push(route.request().postDataJSON());
      if (options.start) return options.start(route, fixture);
      return route.fulfill({ status: 200, contentType: 'application/json',
        body: JSON.stringify({ id: fixture.current.process.id, shell: { ...shell, safe: !!fixture.starts.at(-1).safe } }) });
    }
    const url = new URL(route.request().url());
    fixture.snapshots.push({ id: url.pathname.split('/').at(-1), after: Number(url.searchParams.get('afterSequence')) });
    if (options.read) return options.read(route, fixture);
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(fixture.current) });
  });
  fixture.open = () => fixture.page.locator('button[data-view="shell"]').first().click();
  fixture.output = () => fixture.page.evaluate(() => state.shellBuffer);
  return fixture;
}

async function launch(t) {
  const { chromium } = require(process.env.PLAYWRIGHT_MODULE);
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROME_EXECUTABLE ? { executablePath: process.env.CHROME_EXECUTABLE } : {}) });
  t.after(() => browser.close());
  return browser;
}

test('terminal isolates processes and merges replay/live output exactly once in sequence order', { skip: !enabled }, async t => {
  const fixture = await terminalPage(await launch(t));
  t.after(() => fixture.close());
  await fixture.open();
  await fixture.page.waitForFunction(() => state.shellAttachment?.ready);
  assert.deepEqual(fixture.starts, [{ cwd: fixture.session.projectPath, safe: false, cols: 100, rows: 30 }]);
  assert.ok(fixture.clientMessages.some(message => message.type === 'subscribe'
    && message.processIds?.includes('terminal-1')));
  fixture.send({ type: 'process_output', processId: 'foreign', sequence: 1, data: 'FOREIGN_OUTPUT' });
  fixture.send({ type: 'process_exited', processId: 'foreign', code: 9 });
  fixture.send({ type: 'process_output', processId: 'terminal-1', sequence: 3, data: 'REPEATED\r\n' });
  fixture.send({ type: 'process_output', processId: 'terminal-1', sequence: 2, data: 'REPEATED\r\n' });
  fixture.send({ type: 'process_output', processId: 'terminal-1', sequence: 2, data: 'REPEATED\r\n' });
  await fixture.page.waitForFunction(() => state.shellAttachment.sequence === 3);
  fixture.current = snapshot('terminal-1', [chunk(1, 'INITIAL_PROMPT\r\n'), chunk(2, 'REPEATED\r\n'), chunk(3, 'REPEATED\r\n')]);
  await fixture.page.evaluate(() => refreshShellSnapshot());
  const output = await fixture.output();
  assert.equal(output.match(/INITIAL_PROMPT/g).length, 1);
  assert.equal(output.match(/REPEATED/g).length, 2);
  assert.doesNotMatch(output, /FOREIGN|process exited/);
  assert.equal(await fixture.page.locator('#shell-status-dot').evaluate(node => node.classList.contains('connected')), true);
  await fixture.page.locator('#clear-shell').click();
  await fixture.page.evaluate(() => refreshShellSnapshot());
  assert.equal(await fixture.output(), '');
  fixture.send({ type: 'process_output', processId: 'terminal-1', sequence: 4, data: 'AFTER_CLEAR' });
  await fixture.page.waitForFunction(() => state.shellBuffer === 'AFTER_CLEAR');
  assert.deepEqual(fixture.errors, []);
});

test('terminal recovers output and an exit which arrive before the start response', { skip: !enabled }, async t => {
  const fixture = await terminalPage(await launch(t), { start: async (route, fixture) => {
    fixture.send({ type: 'process_output', processId: 'fast-terminal', sequence: 1, data: 'FAST_FINAL_OUTPUT\r\n' });
    fixture.send({ type: 'process_exited', processId: 'fast-terminal', code: 7 });
    fixture.current = snapshot('fast-terminal', [chunk(1, 'FAST_FINAL_OUTPUT\r\n')], { running: false, exitCode: 7 });
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ id: 'fast-terminal', shell }) });
  } });
  t.after(() => fixture.close());
  await fixture.open();
  await fixture.page.waitForFunction(() => state.shellAttachment?.phase === 'exited');
  assert.match(await fixture.output(), /FAST_FINAL_OUTPUT[\s\S]*process exited: 7/);
  assert.equal(await fixture.page.evaluate(() => state.currentShellProcess), null);
  assert.equal(await fixture.page.locator('#stop-shell').isDisabled(), true);
  await fixture.page.evaluate(() => ensureShellRunningForActiveProject());
  assert.equal(fixture.starts.length, 1, 'an exited shell must not auto-restart in a loop');
  assert.deepEqual(fixture.errors, []);
});

test('terminal waits for the final snapshot when exit overlaps an older running snapshot', { skip: !enabled }, async t => {
  let release;
  const held = new Promise(resolve => { release = resolve; });
  let holdNext = false;
  const fixture = await terminalPage(await launch(t), { read: async (route, fixture) => {
    const body = JSON.stringify(fixture.current);
    if (holdNext) {
      holdNext = false;
      await held;
    }
    await route.fulfill({ status: 200, contentType: 'application/json', body });
  } });
  t.after(() => { release(); return fixture.close(); });
  await fixture.open();
  await fixture.page.waitForFunction(() => state.shellAttachment?.ready);
  holdNext = true;
  await fixture.page.evaluate(() => { window.__terminalRefresh = refreshShellSnapshot(); });
  await fixture.page.waitForFunction(() => state.shellAttachment.syncing);
  fixture.current = snapshot('terminal-1', [chunk(1, 'INITIAL_PROMPT\r\n'), chunk(2, 'FINAL_TAIL\r\n')], { running: false, exitCode: 0 });
  fixture.send({ type: 'process_exited', processId: 'terminal-1', code: 0 });
  await fixture.page.waitForFunction(() => state.shellAttachment.pendingExit !== null);
  release();
  await fixture.page.waitForFunction(() => state.shellAttachment.phase === 'exited');
  assert.match(await fixture.output(), /FINAL_TAIL[\s\S]*process exited: 0/);
  assert.equal((await fixture.output()).match(/process exited/g).length, 1);
  assert.deepEqual(fixture.errors, []);
});

test('selecting another process aborts stale attachment responses and replays the selected process', { skip: !enabled }, async t => {
  let release;
  const held = new Promise(resolve => { release = resolve; });
  const fixture = await terminalPage(await launch(t), { read: async (route, fixture) => {
    const id = new URL(route.request().url()).pathname.split('/').at(-1);
    if (id === 'terminal-1') await held;
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(
      snapshot(id, [chunk(1, id === 'terminal-1' ? 'STALE_OUTPUT' : 'SELECTED_HISTORY')])) });
  } });
  t.after(() => { release(); return fixture.close(); });
  await fixture.open();
  await fixture.page.waitForFunction(() => state.shellAttachment?.syncing);
  await fixture.page.evaluate(() => useShellProcess('selected-terminal'));
  release();
  await settle(fixture.page);
  assert.match(await fixture.output(), /SELECTED_HISTORY/);
  assert.doesNotMatch(await fixture.output(), /STALE_OUTPUT/);
  fixture.send({ type: 'process_output', processId: 'terminal-1', sequence: 2, data: 'STALE_LIVE' });
  await settle(fixture.page);
  assert.doesNotMatch(await fixture.output(), /STALE_LIVE/);
  assert.equal(await fixture.page.evaluate(() => state.currentShellProcess), 'selected-terminal');
  assert.deepEqual(fixture.errors, []);
});

test('safe terminal is an explicit profile-free restart and Stop retains the final tail', { skip: !enabled }, async t => {
  const fixture = await terminalPage(await launch(t));
  t.after(() => fixture.close());
  await fixture.open();
  await fixture.page.waitForFunction(() => state.shellAttachment?.ready);
  fixture.current = snapshot('safe-terminal', [chunk(1, 'SAFE_PROMPT')]);
  assert.match(await fixture.page.locator('#safe-shell').evaluate(node => getComputedStyle(node, '::before').maskImage), /data:image\/svg\+xml/);
  await fixture.page.locator('#safe-shell').click();
  await fixture.page.waitForFunction(() => state.currentShellProcess === 'safe-terminal' && state.shellAttachment.ready);
  assert.equal(fixture.starts.at(-1).safe, true);
  assert.ok(fixture.deletes.includes('terminal-1'));
  assert.match(await fixture.page.locator('#shell-status-label').innerText(), /safe mode/);
  fixture.current = snapshot('safe-terminal', [chunk(1, 'SAFE_PROMPT'), chunk(2, 'STOP_FINAL_TAIL')], { running: false, exitCode: 0 });
  await fixture.page.locator('#stop-shell').click();
  await fixture.page.waitForFunction(() => state.shellAttachment.phase === 'exited');
  assert.match(await fixture.output(), /STOP_FINAL_TAIL[\s\S]*process exited/);
  await fixture.page.waitForFunction(() => document.querySelector('#stop-shell').disabled);
  assert.ok(fixture.deletes.includes('safe-terminal'));
  assert.deepEqual(fixture.errors, []);
});

test('a new WebSocket refreshes the selected terminal without replay duplicates', { skip: !enabled }, async t => {
  const fixture = await terminalPage(await launch(t));
  t.after(() => fixture.close());
  await fixture.open();
  await fixture.page.waitForFunction(() => state.shellAttachment?.ready);
  fixture.current = snapshot('terminal-1', [chunk(1, 'INITIAL_PROMPT\r\n'), chunk(2, 'OFFLINE_OUTPUT\r\n')]);
  await fixture.page.evaluate(() => connectWs());
  await fixture.page.waitForFunction(() => state.shellAttachment.sequence === 2);
  await fixture.stopPolling();
  await fixture.page.evaluate(() => connectWs());
  await fixture.page.waitForFunction(() => state.ws?.readyState === WebSocket.OPEN && !state.shellAttachment.syncing);
  assert.equal((await fixture.output()).match(/INITIAL_PROMPT/g).length, 1);
  assert.equal((await fixture.output()).match(/OFFLINE_OUTPUT/g).length, 1);
  assert.deepEqual(fixture.errors, []);
});

test('HTTP fallback serializes keystrokes instead of racing input requests', { skip: !enabled }, async t => {
  let release;
  const held = new Promise(resolve => { release = resolve; });
  const inputs = [];
  const fixture = await terminalPage(await launch(t), { process: async route => {
    if (new URL(route.request().url()).pathname.endsWith('/input')) {
      inputs.push(route.request().postDataJSON().data);
      if (inputs.length === 1) await held;
    }
    await route.fulfill({ status: 200, contentType: 'application/json', body: '{}' });
  } });
  t.after(() => { release(); return fixture.close(); });
  await fixture.open();
  await fixture.page.waitForFunction(() => state.shellAttachment?.ready);
  await fixture.page.evaluate(() => {
    const socket = state.ws;
    state.ws = null;
    socket.close();
    window.__pendingTerminalInput = Promise.all(['first', 'second', '\r'].map(sendShellInput));
  });
  const deadline = Date.now() + 5000;
  while (inputs.length === 0 && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 20));
  assert.deepEqual(inputs, ['first'], 'later keystrokes wait for the previous HTTP write');
  release();
  await fixture.page.evaluate(() => window.__pendingTerminalInput);
  assert.deepEqual(inputs, ['first', 'second', '\r']);
  assert.deepEqual(fixture.errors, []);
});

test('selecting a process during a pending start cannot attach or leak the stale new process', { skip: !enabled }, async t => {
  let release;
  const held = new Promise(resolve => { release = resolve; });
  const fixture = await terminalPage(await launch(t), { start: async route => {
    await held;
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ id: 'stale-new-process', shell }) });
  }, read: async route => route.fulfill({ status: 200, contentType: 'application/json',
    body: JSON.stringify(snapshot('selected-terminal', [chunk(1, 'SELECTED_EXISTING_HISTORY')])) }) });
  t.after(() => { release(); return fixture.close(); });
  await fixture.open();
  await fixture.page.waitForFunction(() => state.shellStarting);
  await fixture.page.evaluate(() => useShellProcess('selected-terminal'));
  release();
  await fixture.page.waitForFunction(() => !state.shellStarting);
  await fixture.page.waitForFunction(() => state.currentShellProcess === 'selected-terminal');
  const deadline = Date.now() + 5000;
  while (!fixture.deletes.includes('stale-new-process') && Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  assert.ok(fixture.deletes.includes('stale-new-process'));
  assert.match(await fixture.output(), /SELECTED_EXISTING_HISTORY/);
  assert.doesNotMatch(await fixture.output(), /started stale-new-process/);
  assert.deepEqual(fixture.errors, []);
});

test('ring-buffer gaps and expired process IDs are explicit rather than stuck connected', { skip: !enabled }, async t => {
  let expired = false;
  const fixture = await terminalPage(await launch(t), { read: async (route, fixture) => {
    await route.fulfill({ status: expired ? 404 : 200, contentType: 'application/json',
      body: JSON.stringify(expired ? { error: 'process not found' } : fixture.current) });
  } });
  t.after(() => fixture.close());
  fixture.current = snapshot('terminal-1', [chunk(40, 'RETAINED_TAIL')]);
  await fixture.open();
  await fixture.page.waitForFunction(() => state.shellAttachment?.ready);
  assert.match(await fixture.output(), /earlier terminal output is no longer available[\s\S]*RETAINED_TAIL/);
  expired = true;
  await fixture.page.evaluate(() => refreshShellSnapshot());
  assert.match(await fixture.output(), /host restarted/);
  assert.equal(await fixture.page.evaluate(() => state.shellAttachment.phase), 'exited');
  assert.equal(await fixture.page.evaluate(() => state.currentShellProcess), null);
  assert.deepEqual(fixture.errors, []);
});
