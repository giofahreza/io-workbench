const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const { join } = require('node:path');
const { test } = require('node:test');
const { providerServer, until, pause } = require('./fixtures/provider-server.cjs');

const enabled = !!process.env.IOWB_TEST_BINARY && process.platform !== 'win32';
async function request(fixture, path, body, token = fixture.env.IO_WORKBENCH_TOKEN) {
  return fetch(fixture.origin + path, { method: body === undefined ? 'GET' : 'POST',
    headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
}
async function start(fixture, body = {}, token) {
  const response = await request(fixture, '/api/terminal', { cwd: fixture.project, ...body }, token);
  assert.equal(response.status, 200, await response.clone().text());
  return response.json();
}
async function snapshot(fixture, id, after = 0) {
  const response = await request(fixture, `/api/terminal/${id}?afterSequence=${after}`);
  assert.equal(response.status, 200);
  return response.json();
}
async function input(fixture, id, data) {
  const response = await request(fixture, `/api/process/${id}/input`, { data });
  assert.equal(response.status, 200);
}
const text = value => value.chunks.map(chunk => chunk.data).join('');

test('terminal API selects the host shell, reads startup files, accepts input/resize and reconciles exit', {
  skip: !enabled, timeout: 30000,
}, async t => {
  const fixture = await providerServer({ env: { SHELL: '/bin/bash' } });
  t.after(() => fixture.close());
  await fs.writeFile(join(fixture.root, '.bashrc'), "printf 'HOST_STARTUP_LOADED\\n'\n");
  const process = await start(fixture, { cols: 91, rows: 27 });
  assert.deepEqual(process.shell, { command: '/bin/bash', args: [], source: 'environment', safe: false });
  await until(async () => text(await snapshot(fixture, process.id)).includes('HOST_STARTUP_LOADED'), 'startup output');
  const socket = await fixture.websocket();
  socket.socket.send(JSON.stringify({ type: 'subscribe', processIds: [process.id], sessionIds: [], chatSessionIds: [] }));
  await until(() => socket.events.find(event => event.type === 'process_output' && event.processId === process.id), 'terminal replay');
  const resize = await request(fixture, `/api/process/${process.id}/resize`, { cols: 99, rows: 31 });
  assert.equal(resize.status, 200);
  await input(fixture, process.id, "printf 'REAL_%s\\n' INPUT_OK; pwd -P; stty size\n");
  await until(async () => {
    const output = text(await snapshot(fixture, process.id));
    return output.includes('REAL_INPUT_OK') && output.includes(fixture.project) && output.includes('31 99');
  }, 'real command output and terminal size');
  await input(fixture, process.id, 'exit 3\n');
  const final = await until(async () => {
    const value = await snapshot(fixture, process.id);
    return !value.running && value;
  }, 'final terminal status');
  assert.equal(final.exitCode, 3);
  assert.match(text(final), /REAL_INPUT_OK/);
  assert.equal((await snapshot(fixture, process.id, final.lastSequence)).chunks.length, 0);
  assert.equal((await fixture.request('/api/process')).some(value => value.id === process.id), false);
  assert.deepEqual(final.chunks.map(value => value.sequence), [...final.chunks].map(value => value.sequence).sort((left, right) => left - right));
});

test('terminal snapshot retains a fast exit before the client knows its ID, with identical WebSocket identities', {
  skip: !enabled, timeout: 30000,
}, async t => {
  const fixture = await providerServer({ env: {
    SHELL: '/missing/implicit-shell', IO_WORKBENCH_TERMINAL_SHELL: '/bin/sh',
    IO_WORKBENCH_TERMINAL_ARGS: JSON.stringify(['-c', "printf 'FAST_FINAL_TAIL\\n'; exit 7"]),
  } });
  t.after(() => fixture.close());
  const socket = await fixture.websocket();
  const process = await start(fixture);
  assert.equal(process.shell.source, 'configured');
  await pause(250);
  const final = await until(async () => {
    const value = await snapshot(fixture, process.id);
    return !value.running && value;
  }, 'retained fast terminal');
  assert.equal(final.exitCode, 7);
  assert.match(text(final), /FAST_FINAL_TAIL/);
  const live = socket.events.filter(event => event.type === 'process_output' && event.processId === process.id);
  for (const event of live) assert.deepEqual(final.chunks.find(chunk => chunk.sequence === event.sequence),
    { stream: event.stream, data: event.data, sequence: event.sequence });
  const exit = await until(() => socket.events.find(event => event.type === 'process_exited' && event.processId === process.id), 'fast exit event');
  assert.equal(exit.code, 7);
});

test('invalid configured profiles are visible and safe mode bypasses profiles and startup environment hooks', {
  skip: !enabled, timeout: 30000,
}, async t => {
  const fixture = await providerServer({ env: {
    IO_WORKBENCH_TERMINAL_SHELL: '/missing/profile', IO_WORKBENCH_TERMINAL_ARGS: '{bad-json',
    ENV: '/missing/startup-hook', BASH_ENV: '/missing/other-hook', IO_WORKBENCH_TOKEN: 'fixture-host-secret',
  } });
  t.after(() => fixture.close());
  await fs.writeFile(join(fixture.root, '.bashrc'), "printf 'UNSAFE_STARTUP_LOADED\\n'; exit 42\n");
  const invalid = await request(fixture, '/api/terminal', { cwd: fixture.project });
  assert.equal(invalid.status, 400);
  assert.match(await invalid.text(), /JSON array/);
  const process = await start(fixture, { safe: true });
  assert.equal(process.shell.safe, true);
  assert.equal(process.shell.source, 'safe');
  assert.deepEqual(process.shell.args, ['--noprofile', '--norc', '-i']);
  await input(fixture, process.id, "printf 'SAFE_%s|%s|%s|%s\\n' OK \"${ENV-unset}\" \"${BASH_ENV-unset}\" \"${IO_WORKBENCH_TOKEN-unset}\"\n");
  const value = await until(async () => {
    const current = await snapshot(fixture, process.id);
    return text(current).includes('SAFE_OK|unset|unset|unset') && current;
  }, 'safe terminal output');
  assert.doesNotMatch(text(value), /UNSAFE_STARTUP_LOADED|fixture-host-secret|failed to load module/);
  await input(fixture, process.id, 'exit\n');
});

test('terminal routes retain auth and workspace boundaries without changing arbitrary process starts', {
  skip: !enabled, timeout: 30000,
}, async t => {
  const fixture = await providerServer({ auth: true, env: { SHELL: '/bin/sh' } });
  t.after(() => fixture.close());
  assert.equal((await request(fixture, '/api/terminal', { cwd: fixture.project })).status, 401);
  assert.equal((await request(fixture, '/api/terminal/unknown')).status, 401);
  const registration = await request(fixture, '/api/auth/register', { username: 'terminal-test', password: 'terminal-fixture-password' });
  assert.equal(registration.status, 200);
  const { token } = await registration.json();
  assert.equal((await request(fixture, '/api/terminal/unknown', undefined, token)).status, 404);
  const outside = await request(fixture, '/api/terminal', { cwd: '/etc' }, token);
  assert.ok([400, 403].includes(outside.status), `outside directory returns ${outside.status}`);
  const file = join(fixture.root, 'not-a-directory');
  await fs.writeFile(file, 'fixture');
  assert.equal((await request(fixture, '/api/terminal', { cwd: file }, token)).status, 400);
  const arbitrary = await request(fixture, '/api/process', { command: '/bin/sh', args: ['-c', 'exit 0'], pty: false }, token);
  assert.equal(arbitrary.status, 200);
  const process = await start(fixture, { safe: true }, token);
  const stopped = await fetch(`${fixture.origin}/api/process/${process.id}`, { method: 'DELETE', headers: { authorization: `Bearer ${token}` } });
  assert.equal(stopped.status, 200);
});
