const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const { join } = require('node:path');
const { spawn } = require('node:child_process');
const http = require('node:http');

const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(predicate, message = 'condition', milliseconds = 20000) {
  const deadline = Date.now() + milliseconds;
  while (Date.now() < deadline) {
    const value = await predicate();
    if (value) return value;
    await pause(30);
  }
  throw new Error(`Timed out waiting for ${message}`);
}

// Only this script can be selected as a provider. It uses local files and has
// no network client. Credentials, HOME and provider caches belong to this fixture.
const cliSource = `#!${process.execPath}
const fs = require('node:fs');
const path = require('node:path');
const root = process.env.IOWB_FIXTURE_ROOT;
const provider = path.basename(process.argv[1]);
const args = process.argv.slice(2);
const control = JSON.parse(fs.readFileSync(path.join(root, 'control.json')))[provider] || {};
const append = (file, value) => fs.appendFileSync(path.join(root, file), JSON.stringify(value)+'\\n');
const wait = ms => new Promise(resolve => setTimeout(resolve, ms || 0));
(async () => {
  if (args[0] === '--version') {
    append('probes.jsonl', {provider, kind:'version'});
    await wait(control.versionDelay);
    console.error('fake-status-secret');
    console.log(control.version || 'fixture-cli 1.2.3');
    if (control.disappearVersion) fs.unlinkSync(process.argv[1]);
    process.exit(control.versionExit || 0);
  }
  if (args[0] === 'login' || args[0] === 'auth') {
    append('probes.jsonl', {provider, kind:'auth'});
    await wait(control.authDelay);
    console.error('fake-status-secret');
    if (control.disappear) fs.unlinkSync(process.argv[1]);
    if (provider === 'claude') console.log(control.authMalformed ? '{invalid secret}' : JSON.stringify({loggedIn: !!control.auth}));
    process.exit(control.authExit ?? (control.auth ? 0 : 1));
  }
  append('launches.jsonl', {provider, args, gatewayKey: process.env.IOWB_IO_GATEWAY_API_KEY || null,
    anthropicKey: process.env.ANTHROPIC_API_KEY || null, baseUrl: process.env.ANTHROPIC_BASE_URL || null,
    oauth: process.env.CLAUDE_CODE_OAUTH_TOKEN || null});
  await wait(control.runDelay);
  if (control.runError) { console.error(control.runError); process.exit(1); }
  if (provider === 'codex') console.log(JSON.stringify({type:'item.completed',item:{type:'agent_message',text:'Fixture Codex answer'}}));
  else if (provider === 'claude') {
    console.log(JSON.stringify({type:'assistant',message:{content:[{type:'text',text:'Fixture Claude answer'}]}}));
    console.log(JSON.stringify({type:'result',subtype:'success',result:'Fixture Claude answer'}));
  } else console.log(JSON.stringify({type:'message',role:'assistant',content:'Fixture Gemini answer'}));
})();
`;

async function providerServer(options = {}) {
  assert.ok(process.env.IOWB_TEST_BINARY, 'IOWB_TEST_BINARY must name the test build');
  const parent = join(require('node:os').homedir(), '.cache', 'iowb-tests');
  await fs.mkdir(parent, { recursive: true });
  const root = await fs.mkdtemp(join(parent, 'acceptance-'));
  const bin = join(root, '.local/bin');
  const project = join(root, 'project');
  await fs.mkdir(bin, { recursive: true });
  await fs.mkdir(project);
  const control = Object.fromEntries(['codex', 'claude', 'gemini'].map(p => [p, { auth: true }]));
  await fs.writeFile(join(root, 'control.json'), JSON.stringify(control));
  let gatewayMode = 'ok';
  let gatewayModels = [{ id: 'cod:gpt-test' }, { id: 'cld:sonnet-test' }];
  const gatewayRequests = [];
  const held = new Set();
  const gateway = http.createServer((request, response) => {
    gatewayRequests.push({ url: request.url, authorization: request.headers.authorization });
    if (request.headers.authorization !== 'Bearer fake-gateway-key') {
      response.writeHead(401); return response.end('Unauthorized');
    }
    if (gatewayMode === 'hold') { held.add(response); response.once('close', () => held.delete(response)); return; }
    if (gatewayMode === 'malformed') return response.end('{fake-gateway-key');
    if (gatewayMode === 'schema') return response.end(JSON.stringify({wrong: 'fake-gateway-key'}));
    const status = gatewayMode === '401' ? 401 : gatewayMode === '403' ? 403 : 200;
    response.writeHead(status, { 'content-type': 'application/json' });
    response.end(JSON.stringify(status !== 200 ? {error:'fake-gateway-key'} : {data:gatewayMode === 'empty' ? [] : gatewayModels}));
  });
  await new Promise(resolve => gateway.listen(0, '127.0.0.1', resolve));
  const reserve = http.createServer();
  await new Promise(resolve => reserve.listen(0, '127.0.0.1', resolve));
  const port = reserve.address().port;
  await new Promise(resolve => reserve.close(resolve));
  const origin = `http://127.0.0.1:${port}`;
  const sockets = new Set();
  let child, logs = '';
  const env = {
    PATH: options.path || '/usr/bin:/bin', HOME: root, TMPDIR: root, IOWB_FIXTURE_ROOT: root,
    HTTP_PROXY: 'http://127.0.0.1:1', HTTPS_PROXY: 'http://127.0.0.1:1', NO_PROXY: '127.0.0.1,localhost',
    IO_WORKBENCH_HOST: '127.0.0.1', IO_WORKBENCH_PORT: String(port), IO_WORKBENCH_AUTH_REQUIRED: String(!!options.auth),
    IO_WORKBENCH_CONFIG_DIR: join(root, 'config'), IO_WORKBENCH_WORKSPACE_ROOT: root,
    CLAUDE_CODE_OAUTH_TOKEN: 'fake-native-token', ...options.env,
  };
  // Explicit missing paths also prevent discovery of any real installed CLI.
  for (const p of ['codex', 'claude', 'gemini']) env[`IO_WORKBENCH_${p.toUpperCase()}_COMMAND`] = join(bin, p);
  if (options.userLocal) for (const p of ['codex', 'claude', 'gemini']) delete env[`IO_WORKBENCH_${p.toUpperCase()}_COMMAND`];
  const fixture = {
    root, bin, project, origin, env, control, gatewayRequests,
    get logs() { return logs; },
    async start() {
      child = spawn(process.env.IOWB_TEST_BINARY, ['start'], {env, stdio:['ignore','pipe','pipe']});
      child.stdout.on('data', chunk => { logs += chunk; });
      child.stderr.on('data', chunk => { logs += chunk; });
      await until(() => fetch(`${origin}/health`).then(r => r.ok).catch(() => false), 'server health');
    },
    async stop(signal = 'SIGTERM') {
      for (const ws of sockets) ws.socket.close();
      sockets.clear();
      if (child && child.exitCode === null && child.signalCode === null) {
        child.kill(signal);
        await new Promise(resolve => child.once('exit', resolve));
      }
    },
    async update(provider, patch) {
      Object.assign(control[provider], patch);
      await fs.writeFile(join(root, 'control.tmp'), JSON.stringify(control));
      await fs.rename(join(root, 'control.tmp'), join(root, 'control.json'));
      // Explicit refresh is coalesced for one second, just like real Recheck.
      await pause(1050);
    },
    async install(provider) { await fs.writeFile(join(bin, provider), cliSource, {mode:0o700}); },
    async uninstall(provider) { await fs.rm(join(bin, provider), {force:true}); await pause(1050); },
    async records(file = 'launches.jsonl') {
      return (await fs.readFile(join(root, file), 'utf8').catch(() => '')).trim().split('\n').filter(Boolean).map(JSON.parse);
    },
    async request(path, body, token) {
      const response = await fetch(origin + path, {method:body === undefined ? 'GET' : 'PUT',
        headers:{'content-type':'application/json', ...(token ? {authorization:`Bearer ${token}`} : {})},
        ...(body === undefined ? {} : {body:JSON.stringify(body)})});
      assert.equal(response.status, 200, `${path}: ${await response.clone().text()}`);
      return response.json();
    },
    models(provider, params = '', token) { return fixture.request(`/api/chat/models?provider=${provider}&refresh=true${params}`, undefined, token); },
    async configure(runtime = 'io_gateway', extra = {}, token) {
      return fixture.request('/api/settings/direct-ai', {mode:'aiproxy', chatRuntime:runtime,
        gatewayUrl:`http://127.0.0.1:${gateway.address().port}`, gatewayApiKey:'fake-gateway-key', ...extra}, token);
    },
    gateway(mode, models) {
      gatewayMode = mode;
      if (models) gatewayModels = models;
      if (mode !== 'hold') for (const response of held) response.end(JSON.stringify({data:gatewayModels}));
    },
    async websocket(token) {
      const socket = new WebSocket(origin.replace('http:', 'ws:') + '/ws' + (token ? `?token=${token}` : ''));
      const events = [];
      socket.addEventListener('message', event => events.push(JSON.parse(event.data)));
      await new Promise((resolve,reject) => {socket.addEventListener('open',resolve,{once:true});socket.addEventListener('error',reject,{once:true});});
      const ws = {socket, events,
        start(provider, sessionId, runtime, model = '', requestId = `request-${sessionId}-${events.length}`) {
          const offset = events.length;
          socket.send(JSON.stringify({type:'start_session', provider, sessionId, runtime, model, requestId,
            projectPath:project, prompt:'Harmless fixture prompt'}));
          return {requestId, offset, result:() => until(() => events.slice(offset).find(e => e.sessionId === sessionId &&
            (e.type === 'chat_setup_required' || e.type === 'error' || e.type === 'session_status' && ['completed','failed'].includes(e.status))), `result for ${sessionId}`, 30000)};
        },
      };
      sockets.add(ws);
      return ws;
    },
    async close() {
      await fixture.stop();
      gateway.closeAllConnections();
      await new Promise(resolve => gateway.close(resolve));
      await fs.rm(root, {recursive:true,force:true});
    },
  };
  try { await fixture.start(); return fixture; } catch(error) { await fixture.close(); throw error; }
}
module.exports = {providerServer, until, pause};
