// Run after building io-workbench:
// IOWB_TEST_BINARY=$PWD/target/debug/io-workbench node --test crates/iowb-server/tests/provider-setup.test.cjs
const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs/promises');
const { join } = require('node:path');
const { spawn } = require('node:child_process');
const http = require('node:http');

const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(predicate) {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) { const value = await predicate(); if (value) return value; await pause(40); }
  throw new Error('Timed out waiting for the test server');
}

test('running Workbench: first-run setup, both CLI routes, pinned sessions and scoped failures', {
  skip: !process.env.IOWB_TEST_BINARY, timeout: 120000,
}, async () => {
  // Use a workspace-safe path on macOS, where /tmp canonicalizes into /private.
  const tempParent = join(require('node:os').homedir(), '.cache', 'iowb-tests');
  await fs.mkdir(tempParent, { recursive: true });
  const root = await fs.mkdtemp(join(tempParent, 'provider-e2e-'));
  const project = join(root, 'project');
  await fs.mkdir(project);
  let gatewayStatus = 200;
  const gateway = http.createServer((request, response) => {
    assert.equal(request.headers.authorization, 'Bearer fake-gateway-key');
    response.writeHead(gatewayStatus, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ data: [{ id: 'cod:gpt-test' }, { id: 'cld:sonnet-test' }] }));
  });
  await new Promise(resolve => gateway.listen(0, '127.0.0.1', resolve));
  const portReservation = http.createServer();
  await new Promise(resolve => portReservation.listen(0, '127.0.0.1', resolve));
  const port = portReservation.address().port;
  await new Promise(resolve => portReservation.close(resolve));
  const origin = `http://127.0.0.1:${port}`;
  let logs = '';
  const child = spawn(process.env.IOWB_TEST_BINARY, ['start'], { env: {
    PATH: process.env.PATH, HOME: root, TMPDIR: root,
    HTTP_PROXY: 'http://127.0.0.1:1', HTTPS_PROXY: 'http://127.0.0.1:1', NO_PROXY: '127.0.0.1,localhost',
    IO_WORKBENCH_HOST: '127.0.0.1', IO_WORKBENCH_PORT: String(port), IO_WORKBENCH_AUTH_REQUIRED: 'false',
    IO_WORKBENCH_CONFIG_DIR: join(root, 'config'), IO_WORKBENCH_WORKSPACE_ROOT: root,
    IO_WORKBENCH_CODEX_COMMAND: join(root, 'codex'), IO_WORKBENCH_CLAUDE_COMMAND: join(root, 'claude'),
    IO_WORKBENCH_GEMINI_COMMAND: join(root, 'gemini'),
    CLAUDE_CODE_OAUTH_TOKEN: 'fake-native-token',
  }, stdio: ['ignore', 'pipe', 'pipe'] });
  child.stdout.on('data', data => { logs += data; });
  child.stderr.on('data', data => { logs += data; });
  let ws;
  const events = [];
  const request = async (path, body) => {
    const response = await fetch(origin + path, body ? { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) } : {});
    assert.equal(response.status, 200, `${path}: ${await response.clone().text()}`);
    return response.json();
  };
  const modelQuery = (provider, extra = '') => request(`/api/chat/models?provider=${provider}&refresh=true${extra}`);
  const launchRecords = async () => (await fs.readFile(join(root, 'launches.jsonl'), 'utf8').catch(() => '')).trim().split('\n').filter(Boolean).map(JSON.parse);
  const send = async (provider, id, requestId, runtime, model) => {
    ws.send(JSON.stringify({ type: 'start_session', provider, sessionId: id, requestId, runtime, model, projectPath: project, prompt: 'Harmless fake test prompt' }));
    return until(() => events.find(event => event.sessionId === id && (event.type === 'chat_setup_required' || event.type === 'session_status' && ['completed', 'failed'].includes(event.status))));
  };
  try {
    await until(async () => fetch(origin + '/health').then(r => r.ok).catch(() => false));
    for (const provider of ['codex', 'claude', 'gemini']) {
      const missing = await modelQuery(provider);
      assert.equal(missing.problem.code, 'provider_cli_unavailable');
      assert.deepEqual(missing.models, []);
      assert.match(missing.readiness.installUrl, /^https:\/\//);
    }
    const fakeSource = `#!${process.execPath}
const fs = require('node:fs');
const path = require('node:path');
const root = path.dirname(process.argv[1]);
const provider = path.basename(process.argv[1]);
const args = process.argv.slice(2);
fs.appendFileSync(path.join(root, 'checks.jsonl'), JSON.stringify({provider,args})+'\\n');
if (args[0] === '--version') { console.log('test-cli 1.2.3'); process.exit(0); }
if (args[0] === 'login' || args[0] === 'auth') {
  const loggedIn = fs.existsSync(path.join(root, 'authenticated'));
  if (provider === 'codex' && fs.existsSync(path.join(root, 'disappear-on-probe'))) fs.unlinkSync(process.argv[1]);
  if (provider === 'claude') console.log(JSON.stringify({loggedIn}));
  process.exit(loggedIn ? 0 : 1);
}
fs.appendFileSync(path.join(root, 'launches.jsonl'), JSON.stringify({provider,args,
  gatewayKey: process.env.IOWB_IO_GATEWAY_API_KEY || process.env.ANTHROPIC_API_KEY || null,
  baseUrl: process.env.ANTHROPIC_BASE_URL || null, oauth: process.env.CLAUDE_CODE_OAUTH_TOKEN || null})+'\\n');
if (provider === 'codex') console.log(JSON.stringify({type:'item.completed',item:{type:'agent_message',text:'Fake Codex answer'}}));
else { console.log(JSON.stringify({type:'assistant',message:{content:[{type:'text',text:'Fake Claude answer'}]}})); console.log(JSON.stringify({type:'result',subtype:'success',result:'Fake Claude answer'})); }
`;
    for (const provider of ['codex', 'claude']) await fs.writeFile(join(root, provider), fakeSource, { mode: 0o700 });
    await pause(1100);
    for (const provider of ['codex', 'claude']) {
      const unauthenticated = await modelQuery(provider);
      assert.equal(unauthenticated.readiness.installed, true);
      assert.equal(unauthenticated.problem.code, 'provider_auth_required');
      assert.deepEqual(unauthenticated.models, []);
    }
    ws = new WebSocket(origin.replace('http:', 'ws:') + '/ws');
    ws.addEventListener('message', event => events.push(JSON.parse(event.data)));
    await new Promise((resolve, reject) => { ws.addEventListener('open', resolve, { once: true }); ws.addEventListener('error', reject, { once: true }); });
    const missingAuth = await send('codex', 'blocked-auth', 'request-auth', 'native_cli', 'gpt-test');
    assert.equal(missingAuth.code, 'provider_auth_required'); assert.equal(missingAuth.requestId, 'request-auth');
    assert.equal((await launchRecords()).length, 0);
    await fs.writeFile(join(root, 'authenticated'), 'fake');
    await pause(1100);
    const beforeChecks = (await fs.readFile(join(root, 'checks.jsonl'), 'utf8')).trim().split('\n').length;
    const [native] = await Promise.all(Array.from({length: 12}, () => modelQuery('codex')));
    const afterChecks = (await fs.readFile(join(root, 'checks.jsonl'), 'utf8')).trim().split('\n').length;
    assert.equal(afterChecks - beforeChecks, 2, 'concurrent Recheck requests share one version/auth probe');
    assert.equal(native.ready, true); assert.equal(native.models[0].value, '');
    await send('codex', 'native-chat', 'request-native', 'native_cli', '');
    let records = await launchRecords();
    assert.equal(records.length, 1); assert.equal(records[0].gatewayKey, null);
    assert.ok(!records[0].args.some(arg => arg.includes('iowb_gateway')));
    await fs.writeFile(join(root, 'disappear-on-probe'), 'fake');
    await pause(1100);
    const disappeared = await send('codex', 'disappeared-cli', 'request-disappeared', 'native_cli', '');
    assert.equal(disappeared.code, 'provider_cli_unavailable');
    assert.equal(disappeared.requestId, 'request-disappeared');
    await fs.rm(join(root, 'disappear-on-probe'));
    await fs.writeFile(join(root, 'codex'), fakeSource, { mode: 0o700 });
    await request('/api/settings/direct-ai', { mode: 'aiproxy', chatRuntime: 'io_gateway', gatewayUrl: `http://127.0.0.1:${gateway.address().port}`, gatewayApiKey: 'fake-gateway-key' });
    const pinned = await modelQuery('codex', '&sessionId=native-chat');
    assert.equal(pinned.runtime, 'native_cli');
    await send('codex', 'native-chat', 'request-mismatch', 'io_gateway', 'cod:gpt-test');
    // The prior terminal event is also retained in the event collector.
    const scoped = await until(() => events.find(e => e.type === 'chat_setup_required' && e.requestId === 'request-mismatch'));
    assert.equal(scoped.code, 'chat_runtime_mismatch');
    assert.equal((await launchRecords()).length, 1);
    await fs.rm(join(root, 'authenticated'));
    await pause(1100);
    for (const provider of ['codex', 'claude']) {
      const catalog = await modelQuery(provider);
      assert.equal(catalog.ready, true); assert.equal(catalog.readiness.authenticated, false);
      const id = `gateway-${provider}`;
      const result = await send(provider, id, `request-${id}`, 'io_gateway', provider === 'codex' ? 'cod:gpt-test' : 'cld:sonnet-test');
      assert.notEqual(result.type, 'chat_setup_required');
      const snapshot = await request(`/api/sessions/${id}/snapshot?limit=10`);
      assert.equal(snapshot.session.runtime, 'io_gateway');
    }
    records = await launchRecords();
    const codex = records.find(r => r.provider === 'codex' && r.gatewayKey);
    assert.ok(codex.args.some(arg => arg.includes('iowb_gateway')));
    assert.ok(codex.args.some(arg => arg.includes('wire_api="responses"')));
    const claude = records.find(r => r.provider === 'claude');
    assert.equal(claude.gatewayKey, 'fake-gateway-key'); assert.equal(claude.oauth, null);
    assert.match(claude.baseUrl, /\/claude$/);
    await request('/api/settings/direct-ai', { mode: 'off', chatRuntime: 'native_cli', gatewayUrl: `http://127.0.0.1:${gateway.address().port}` });
    await send('claude', 'gateway-claude', 'request-disabled', 'native_cli', 'cld:sonnet-test');
    const disabledProblem = await until(() => events.find(e => e.requestId === 'request-disabled'));
    assert.equal(disabledProblem.code, 'gateway_configuration_required');
    assert.equal((await launchRecords()).length, 3);
    await request('/api/settings/direct-ai', { mode: 'aiproxy', chatRuntime: 'io_gateway', gatewayUrl: `http://127.0.0.1:${gateway.address().port}` });
    gatewayStatus = 401;
    const rejected = await modelQuery('codex');
    assert.equal(rejected.problem.code, 'gateway_auth_required'); assert.deepEqual(rejected.models, []);
    assert.ok(!JSON.stringify(rejected).includes('fake-gateway-key'));
    assert.ok(!logs.includes('fake-gateway-key'));
  } finally {
    ws?.close(); child.kill('SIGTERM'); gateway.close();
    await new Promise(resolve => child.exitCode !== null ? resolve() : child.once('exit', resolve));
    await fs.rm(root, { recursive: true, force: true });
  }
});
