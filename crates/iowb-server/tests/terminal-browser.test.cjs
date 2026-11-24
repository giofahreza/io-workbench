const assert = require('node:assert/strict');
const { test } = require('node:test');
const { providerServer, until, pause } = require('./fixtures/provider-server.cjs');

const enabled = !!process.env.IOWB_TEST_BINARY && !!process.env.PLAYWRIGHT_MODULE && process.platform !== 'win32';

async function openTerminal(t, env) {
  const fixture = await providerServer({ env });
  t.after(() => fixture.close());
  const created = await fetch(`${fixture.origin}/api/projects/create`, { method: 'POST',
    headers: { 'content-type': 'application/json' }, body: JSON.stringify({ path: fixture.project }) });
  assert.equal(created.status, 200);
  const { chromium } = require(process.env.PLAYWRIGHT_MODULE);
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROME_EXECUTABLE ? { executablePath: process.env.CHROME_EXECUTABLE } : {}) });
  t.after(() => browser.close());
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 }, serviceWorkers: 'block' });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    localStorage.setItem('iowb.lastView', 'chat');
    window.addEventListener('iowb:app-ready', () => { window.__terminalAppReady = true; });
  });
  await page.goto(fixture.origin, { waitUntil: 'networkidle' });
  await page.waitForFunction(() => window.__terminalAppReady && state.ws?.readyState === WebSocket.OPEN);
  await page.evaluate(project => setActiveProject(project), fixture.project);
  await page.locator('button[data-view="shell"]').first().click();
  return { fixture, context, page, errors };
}

async function typeCommand(page, command) {
  await page.locator('#shell-output .xterm-helper-textarea').focus();
  await page.keyboard.type(command);
  await page.keyboard.press('Enter');
}

test('actual browser and host PTY support command input, process isolation, reconnect, safe restart and exit', {
  skip: !enabled, timeout: 60000,
}, async t => {
  const { fixture, context, page, errors } = await openTerminal(t, { SHELL: '/bin/sh', PS1: 'IOWB_TEST> ' });
  await page.waitForFunction(() => state.shellAttachment?.ready && state.shellBuffer.includes('IOWB_TEST>'));
  const processId = await page.evaluate(() => state.currentShellProcess);
  await typeCommand(page, "printf 'BROWSER_%s\\n' INPUT_OK; pwd -P");
  await page.waitForFunction(project => state.shellBuffer.includes('BROWSER_INPUT_OK') && state.shellBuffer.includes(project), fixture.project);
  const foreign = await fetch(`${fixture.origin}/api/process`, { method: 'POST',
    headers: { 'content-type': 'application/json' }, body: JSON.stringify({ command: '/bin/sh',
      args: ['-c', "printf 'FOREIGN_NOISE'; exit 9"], pty: true, cwd: fixture.project }) });
  assert.equal(foreign.status, 200);
  await pause(250);
  assert.equal(await page.evaluate(() => state.currentShellProcess), processId);
  assert.doesNotMatch(await page.evaluate(() => state.shellBuffer), /FOREIGN_NOISE|process exited/);
  assert.equal((await page.evaluate(() => state.shellBuffer)).match(/BROWSER_INPUT_OK/g).length, 1);
  await page.evaluate(() => connectWs());
  await page.waitForFunction(() => state.ws?.readyState === WebSocket.OPEN && state.shellAttachment?.ready && !state.shellAttachment.syncing);
  assert.equal((await page.evaluate(() => state.shellBuffer)).match(/BROWSER_INPUT_OK/g).length, 1, 'reconnect does not duplicate replay');
  await context.routeWebSocket('**/ws**', socket => socket.close({ code: 1013, reason: 'test HTTP fallback' }));
  await page.evaluate(() => state.ws.close());
  await page.waitForFunction(() => !state.ws || state.ws.readyState !== WebSocket.OPEN);
  await typeCommand(page, "printf 'HTTP_%s\\n' FALLBACK_OK");
  await page.waitForFunction(() => state.shellBuffer.includes('HTTP_FALLBACK_OK'), null, { timeout: 12000 }).catch(async error => {
    throw new Error(`${error.message}\nTerminal output: ${await page.evaluate(() => state.shellBuffer)}`);
  });
  await page.locator('#clear-shell').click();
  await page.evaluate(() => refreshShellSnapshot());
  assert.equal(await page.evaluate(() => state.shellBuffer), '');
  await page.locator('#safe-shell').click();
  await page.waitForFunction(previous => state.shellAttachment?.ready && state.shellAttachment.safe
    && state.currentShellProcess !== previous, processId);
  await typeCommand(page, "printf 'SAFE_%s\\n' BROWSER_OK");
  await page.waitForFunction(() => state.shellBuffer.includes('SAFE_BROWSER_OK'));
  await typeCommand(page, 'exit 6');
  await page.waitForFunction(() => state.shellAttachment?.phase === 'exited');
  assert.match(await page.locator('#shell-status-label').innerText(), /exited/i);
  assert.match(await page.evaluate(() => state.shellBuffer), /process exited: 6/);
  assert.equal(await page.locator('#stop-shell').isDisabled(), true);
  await page.evaluate(() => ensureShellRunningForActiveProject());
  assert.equal(await page.evaluate(() => state.currentShellProcess), null);
  await until(async () => (await fixture.request('/api/process')).length === 0, 'all terminal processes end');
  assert.deepEqual(errors, []);
});

test('actual browser can explicitly recover from an unusable host shell profile with a safe terminal', {
  skip: !enabled, timeout: 30000,
}, async t => {
  const { page, errors } = await openTerminal(t, { IO_WORKBENCH_TERMINAL_SHELL: '/missing/broken-profile' });
  await page.waitForFunction(() => !state.shellStarting && document.querySelector('#toast-stack').textContent.includes('safe terminal'));
  assert.equal(await page.evaluate(() => state.currentShellProcess), null);
  await page.locator('#safe-shell').click();
  await page.waitForFunction(() => state.shellAttachment?.ready && state.shellAttachment.safe);
  await typeCommand(page, "printf 'RECOVERY_%s\\n' OK");
  await page.waitForFunction(() => state.shellBuffer.includes('RECOVERY_OK'));
  await page.locator('#stop-shell').click();
  await page.waitForFunction(() => state.shellAttachment.phase === 'exited' && document.querySelector('#stop-shell').disabled);
  assert.deepEqual(errors, []);
});
