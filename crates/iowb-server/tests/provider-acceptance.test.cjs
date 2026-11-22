const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs/promises');
const { join } = require('node:path');
const { execFileSync } = require('node:child_process');
const { providerServer, until, pause } = require('./fixtures/provider-server.cjs');

const enabled = {skip:!process.env.IOWB_TEST_BINARY, timeout:180000};
const providers = ['codex','claude'];
const gatewayModel = p => p === 'codex' ? 'cod:gpt-test' : 'cld:sonnet-test';
async function completed(attempt) {
  const result = await attempt.result();
  assert.equal(result.type, 'session_status', JSON.stringify(result));
  assert.equal(result.status, 'completed', JSON.stringify(result));
}
async function setup(attempt, code) {
  const result = await attempt.result();
  assert.equal(result.type, 'chat_setup_required', JSON.stringify(result));
  assert.equal(result.requestId, attempt.requestId);
  assert.equal(result.code, code, JSON.stringify(result));
  return result;
}

test('RT-003/004/005/006/007/008/010/011/014/025 PR-031/033 MC-014: saved routes, credentials, restart and legacy migration', enabled, async t => {
  const f = await providerServer();
  try {
    for (const p of providers) await f.install(p);
    await pause(1050);
    let ws = await f.websocket();
    for (const p of providers) await t.test(`new native ${p} inherits default`, async () => {
      await completed(ws.start(p, `native-${p}`, undefined));
      const snapshot = await f.request(`/api/sessions/native-${p}/snapshot?limit=10`);
      assert.equal(snapshot.session.runtime, 'native_cli');
    });
    await f.configure();
    for (const p of providers) await t.test(`existing native ${p} continues after gateway enabled`, async () => {
      await completed(ws.start(p, `native-${p}`, 'io_gateway'));
      const launch = (await f.records()).at(-1);
      assert.equal(launch.provider, p);
      assert.equal(launch.gatewayKey, null); assert.equal(launch.anthropicKey, null);
      assert.ok(!launch.args.some(arg => arg.includes('iowb_gateway')));
      assert.equal((await f.request(`/api/sessions/native-${p}/snapshot?limit=10`)).session.runtime, 'native_cli');
      const before = (await f.records()).length;
      const requests = f.gatewayRequests.length;
      await setup(ws.start(p, `native-${p}`, 'io_gateway', gatewayModel(p)), 'chat_runtime_mismatch');
      assert.equal((await f.records()).length, before);
      assert.equal(f.gatewayRequests.length, requests, 'mismatch does not contact gateway');
    });
    for (const p of providers) await t.test(`new and resumed gateway ${p} inherit and preserve route`, async () => {
      await completed(ws.start(p, `gateway-${p}`, undefined, gatewayModel(p)));
      await f.configure('native_cli');
      await completed(ws.start(p, `gateway-${p}`, 'native_cli', gatewayModel(p)));
      const launch = (await f.records()).at(-1);
      assert.equal(launch.provider, p);
      if (p === 'codex') { assert.equal(launch.gatewayKey,'fake-gateway-key'); assert.ok(launch.args.some(a=>a.includes('iowb_gateway'))); }
      else { assert.equal(launch.anthropicKey,'fake-gateway-key'); assert.equal(launch.oauth,null); }
      await f.configure();
    });
    await f.stop();
    // Seed old metadata through the actual persisted database, while stopped.
    execFileSync('/usr/bin/python3', ['-c', `import sqlite3,json,sys
from pathlib import Path
db=next(Path(sys.argv[1]).glob('*.db'))
c=sqlite3.connect(db)
for provider in ('codex','claude'):
 row=c.execute('SELECT metadata FROM sessions WHERE id=?',('gateway-'+provider,)).fetchone()
 meta=json.loads(row[0]);meta.pop('runtime',None)
 c.execute('UPDATE sessions SET metadata=? WHERE id=?',(json.dumps(meta),'gateway-'+provider))
 row=c.execute('SELECT metadata FROM sessions WHERE id=?',('native-'+provider,)).fetchone()
 meta=json.loads(row[0]);meta['runtime']='old-unknown-runtime'
 c.execute('UPDATE sessions SET metadata=? WHERE id=?',(json.dumps(meta),'native-'+provider))
c.commit()`, join(f.root,'config')]);
    await f.start(); ws = await f.websocket();
    for (const p of providers) await t.test(`restart/reopen and legacy ${p} inferred route persists on resume`, async () => {
      assert.equal((await f.models(p, `&sessionId=native-${p}`)).runtime, 'native_cli');
      assert.equal((await f.models(p, `&sessionId=gateway-${p}`)).runtime, 'io_gateway');
      await completed(ws.start(p, `native-${p}`, undefined));
      await completed(ws.start(p, `gateway-${p}`, undefined, gatewayModel(p)));
      assert.equal((await f.request(`/api/sessions/gateway-${p}/snapshot?limit=10`)).session.runtime,'io_gateway');
    });
    // Reopen the now-migrated database again, with valid explicit labels.
    await f.stop();await f.start();ws=await f.websocket();
    for(const p of providers) {
      assert.equal((await f.models(p,`&sessionId=native-${p}`)).runtime,'native_cli');
      assert.equal((await f.models(p,`&sessionId=gateway-${p}`)).runtime,'io_gateway');
    }
    await f.configure('native_cli', {mode:'off'});
    for (const p of providers) await t.test(`disabled saved gateway ${p} is never downgraded`, async () => {
      const before = (await f.records()).length;
      const error = await setup(ws.start(p, `gateway-${p}`, 'native_cli', gatewayModel(p)), 'gateway_configuration_required');
      assert.equal(error.action,'gateway_settings');
      assert.equal((await f.records()).length,before);
    });
    assert.ok(!f.logs.includes('fake-gateway-key'));
    assert.ok(!f.logs.includes('fake-status-secret'));
  } finally { await f.close(); }
});

test('PR-001..024 MC-002/005/009/010/011/015/016 ER-010/011/012/017: real readiness and first-send failure matrix', enabled, async t => {
  const f = await providerServer({userLocal:true});
  try {
    const ws = await f.websocket();
    for (const p of [...providers,'gemini']) await t.test(`${p}: missing, recheck, install, auth, version and spawn path`, async () => {
      for (let n=0;n<2;n++) {
        const status = await f.models(p);
        assert.equal(status.problem.code,'provider_cli_unavailable'); assert.deepEqual(status.models,[]);
        assert.match(status.problem.docsUrl,/^https:\/\//);
        assert.equal(status.readiness.nodeAvailable,false); assert.equal(status.readiness.npmAvailable,false);
        assert.match(status.readiness.scope,/user running.*service/);
      }
      await setup(ws.start(p,`missing-${p}`,'native_cli'),'provider_cli_unavailable');
      await f.update(p,{auth:false}); await f.install(p);
      if(p !== 'gemini') {
        const directory=join(f.root,p==='codex'?'.codex':'.claude');await fs.mkdir(directory,{recursive:true});
        await fs.writeFile(join(directory,p==='codex'?'auth.json':'.credentials.json'),JSON.stringify({access_token:'fake-expired-token',expires_at:1}));
      }
      await f.update(p,{auth:false});
      const unauth = await f.models(p);
      assert.equal(unauth.readiness.installed,true); assert.equal(unauth.problem.code,'provider_auth_required');
      assert.match(unauth.problem.docsUrl,/^https:\/\//);
      await setup(ws.start(p,`auth-${p}`,'native_cli'),'provider_auth_required');
      await f.update(p,{auth:true});
      if (p === 'gemini') {
        await fs.mkdir(join(f.root,'.gemini'),{recursive:true});
        await fs.writeFile(join(f.root,'.gemini/oauth_creds.json'),'{invalid}');
        assert.equal((await f.models(p)).ready,false);
        await fs.writeFile(join(f.root,'.gemini/oauth_creds.json'),JSON.stringify({refresh_token:'fake-token',expiry_date:1}));
        await pause(1050);
      }
      const ready = await f.models(p);
      assert.equal(ready.ready,true); assert.equal(ready.models[0].label,'Native CLI default');
      if(p === 'gemini') { assert.equal(ready.readiness.authState,'configured_unverified');assert.equal(ready.readiness.authenticated,false); }
      await completed(ws.start(p,`installed-${p}`,'native_cli'));
      assert.equal((await f.records()).at(-1).provider,p);
      await f.update(p,{version:'fixture-cli 2.3.4'});
      assert.equal((await f.models(p)).readiness.version,'2.3.4');
      await f.update(p,{versionExit:1});
      assert.equal((await f.models(p)).problem.code,'provider_cli_unavailable');
      await f.update(p,{versionExit:0,version:'no-compatible-version'});
      assert.equal((await f.models(p)).problem.code,'provider_cli_unavailable');
      await f.update(p,{version:'fixture-cli 1.2.3'});
      await fs.chmod(join(f.bin,p),0o600); await pause(1050);
      assert.equal((await f.models(p)).problem.code,'provider_cli_unavailable');
      await fs.chmod(join(f.bin,p),0o700); await pause(1050);
    });
    await t.test('PR-011/ER-011: mixed-provider probes are coalesced and correctly scoped',async()=>{
      await pause(1050);
      const before = (await f.records('probes.jsonl')).length;
      const results = await Promise.all(Array.from({length:6},()=>[...providers,'gemini']).flat().map(p=>f.models(p)));
      assert.ok(results.every(r=>r.ready && r.readiness.version === '1.2.3'));
      const probes=(await f.records('probes.jsonl')).slice(before);
      for(const p of [...providers,'gemini']) {
        assert.equal(probes.filter(x=>x.provider===p && x.kind==='version').length,1);
        assert.equal(probes.filter(x=>x.provider===p && x.kind==='auth').length,p==='gemini'?0:1);
      }
    });
    for(const p of providers)assert.equal((await f.models(p,'&runtime=io_gateway')).problem.code,'gateway_configuration_required');
    await f.configure();
    for (const p of providers) await t.test(`${p}: bad/empty/malformed catalogs and missing configuration`,async()=>{
      for(const [mode,code] of [['401','gateway_auth_required'],['403','gateway_auth_required'],['empty','gateway_catalog_empty'],['malformed','gateway_catalog_unavailable'],['schema','gateway_catalog_unavailable']]) {
        f.gateway(mode);
        const result=await f.models(p,'&runtime=io_gateway');
        assert.equal(result.problem.code,code);assert.deepEqual(result.models,[]);
        assert.ok(!JSON.stringify(result).includes('fake-gateway-key'));
      }
      f.gateway('ok',[{id:gatewayModel(p),label:'First'},{id:gatewayModel(p).toUpperCase(),label:'Duplicate'},{id:'cod:hidden',visibility:'hide'}]);
      const first=await f.models(p,'&runtime=io_gateway'),second=await f.models(p,'&runtime=io_gateway');
      assert.equal(first.models.length,1); assert.deepEqual(first.models,second.models);
      assert.deepEqual(first.models,[{value:gatewayModel(p),label:gatewayModel(p)}]);
      await f.configure('io_gateway',{gatewayUrl:'',baseUrl:''});
      assert.equal((await f.models(p,'&runtime=io_gateway')).problem.code,'gateway_configuration_required');
      await f.configure();
    });
    f.gateway('ok',[{id:'cod:gpt-test'},{id:'cld:sonnet-test'}]);
    for(const p of providers) await t.test(`${p}: invalidated key between catalog and send`,async()=>{
      assert.equal((await f.models(p,'&runtime=io_gateway')).ready,true);
      f.gateway('401'); const before=(await f.records()).length;
      await setup(ws.start(p,`bad-key-${p}`,'io_gateway',gatewayModel(p)),'gateway_auth_required');
      assert.equal((await f.records()).length,before);
      await completed(ws.start(p,`native-unaffected-${p}`,'native_cli'));
      f.gateway('ok');
    });
    await t.test('MC-016: unsupported Gemini gateway send cannot launch or alter another provider',async()=>{
      const before=(await f.records()).length;
      await setup(ws.start('gemini','unsupported-gemini','io_gateway','cld:sonnet-test'),'chat_route_unavailable');
      assert.equal((await f.records()).length,before);
      await completed(ws.start('codex','after-gemini','native_cli'));
    });
    for(const p of providers) await t.test(`${p}: executable removed after preflight`,async()=>{
      await f.update(p,{disappear:true});
      await setup(ws.start(p,`disappeared-${p}`,'native_cli'),'provider_cli_unavailable');
      await f.install(p);await f.update(p,{disappear:false});
    });
    await t.test('Gemini: executable removed after preflight',async()=>{
      await f.update('gemini',{disappearVersion:true});
      await setup(ws.start('gemini','disappeared-gemini','native_cli'),'provider_cli_unavailable');
      await f.install('gemini');await f.update('gemini',{disappearVersion:false});
    });
    await t.test('PR-010/016/018 ER-012: hangs are bounded and independent',async()=>{
      for(const p of [...providers,'gemini']) {
        await f.update(p,{versionDelay:30000});
        const start=Date.now();const version=f.models(p,'&runtime=native_cli');
        const other=p==='codex'?'claude':'codex';
        assert.equal((await f.models(other,'&runtime=native_cli')).ready,true);
        assert.equal((await f.request('/health')).status,'ok');
        const result=await version;
        assert.equal(result.problem.code,'provider_cli_unavailable');assert.ok(Date.now()-start<14000);
        await f.update(p,{versionDelay:0});
      }
      for(const p of providers) {
        await f.update(p,{authDelay:30000});
        const auth=await f.models(p,'&runtime=native_cli');
        assert.equal(auth.ready,false);assert.equal(auth.problem.action,'authenticate');
        await f.update(p,{authDelay:0});
      }
      f.gateway('hold');
      const discovery=f.models('codex','&runtime=io_gateway');
      assert.equal((await f.models('claude','&runtime=native_cli')).ready,true);
      await completed(ws.start('claude','during-gateway-timeout','native_cli'));
      assert.equal((await discovery).problem.code,'gateway_catalog_unavailable');
      f.gateway('ok');
    });
    assert.ok(!f.logs.includes('fake-status-secret'));assert.ok(!f.logs.includes('fake-gateway-key'));
    assert.equal((await f.records()).some(r=>r.args.some(a=>a==='install'||a==='login')),false,'no installation/login launched');
  } finally { await f.close(); }
});

test('RT-015 PR-032 ER-016: concurrent chats, setting changes during run and real process interruption',enabled,async t=>{
  const f=await providerServer();
  try {
    for(const p of providers) await f.install(p);
    let ws=await f.websocket();
    await f.update('claude',{runDelay:1800});
    const claude=ws.start('claude','parallel-claude','native_cli');
    await until(async()=> (await f.records()).some(r=>r.provider==='claude'),'Claude launch');
    await f.configure();
    await setup(ws.start('codex','parallel-codex','native_cli','cod:gpt-test'),'chat_runtime_mismatch');
    await completed(claude);
    const launch=(await f.records()).find(r=>r.provider==='claude');
    assert.equal(launch.anthropicKey,null);
    assert.equal((await f.request('/api/sessions/parallel-claude/snapshot?limit=10')).session.runtime,'native_cli');
    await completed(ws.start('codex','after-setting-change',undefined,'cod:gpt-test'));
    await f.update('claude',{runDelay:30000});
    ws.start('claude','interrupted-chat','native_cli');
    await until(async()=> (await f.records()).filter(r=>r.provider==='claude').length===2,'interrupted launch');
    await f.stop('SIGKILL');
    await f.update('claude',{runDelay:0,runError:'Fixture recovery failure'});
    await f.start();ws=await f.websocket();
    const snapshot=await until(async()=>{
      const snapshot=await f.request('/api/sessions/interrupted-chat/snapshot?limit=10');
      return !snapshot.session.active && snapshot;
    },'startup reconciliation');
    assert.ok(snapshot.messages.some(m=>/restart|interrupt|recovery failure/i.test(m.content)));
    await f.update('claude',{runDelay:0,runError:null});
    await completed(ws.start('claude','new-after-restart','native_cli'));
    assert.equal(ws.events.some(e=>e.type==='chat_setup_required' && e.sessionId==='new-after-restart'),false);
  } finally {await f.close();}
});

test('RT-020/021/022 ER-007/008 PR-008/027/034: legacy errors, service scope, no-credential tools and supported account boundary',enabled,async()=>{
  const f=await providerServer();
  const post=async(path,body)=>{
    const r=await fetch(f.origin+path,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(body)});
    assert.equal(r.status,200,await r.clone().text());return r.json();
  };
  try {
    // A CLI installed for another HOME does not configure the service user.
    const otherHome=join(f.root,'ssh-user/.local/bin');await fs.mkdir(otherHome,{recursive:true});
    await fs.writeFile(join(otherHome,'codex'),'#!/bin/sh\necho 1.2.3\n',{mode:0o700});
    assert.equal((await f.models('codex')).readiness.installed,false);
    const project=await post('/api/projects/create',{path:f.project});
    assert.ok(project.path);
    for(const path of ['/api/projects',`/api/projects/${encodeURIComponent(f.project)}/files`,'/api/database','/api/database/connections','/api/process','/api/settings']) {
      const response=await fetch(f.origin+path);assert.equal(response.status,200,`${path}: ${await response.text()}`);
    }
    execFileSync('/usr/bin/git',['init',f.project],{env:{PATH:'/usr/bin:/bin',HOME:f.root,GIT_CONFIG_GLOBAL:'/dev/null',GIT_CONFIG_NOSYSTEM:'1'},stdio:'ignore'});
    assert.equal((await fetch(`${f.origin}/api/git/status?projectPath=${encodeURIComponent(f.project)}`)).status,200);
    const process=await post('/api/process',{command:'/bin/sh',args:['-c','printf harmless-terminal-check'],cwd:f.project});
    assert.ok(process.id);
    const ws=await f.websocket();
    const legacy=ws.start('codex','legacy-no-request','native_cli','cod:gpt-test',null);
    const event=await legacy.result();assert.equal(event.type,'error');assert.equal(event.sessionId,'legacy-no-request');
    assert.equal(ws.events.some(e=>e.type==='chat_setup_required' && e.sessionId==='legacy-no-request'),false);
    for(const p of providers) {
      await f.install(p);
      for(const text of ['similar unsupported model failure','model is not supported when using Codex with a ChatGPT account']) {
        await f.update(p,{runError:text});
        const attempt=ws.start(p,`generic-${p}-${text.length}`,'native_cli');
        await attempt.result();
        await until(()=>ws.events.slice(attempt.offset).some(e=>(e.type==='error' && (e.message+' '+e.details).includes(text)) || (e.type==='output' && e.content.includes(text))),'raw error delivery');
        assert.equal(ws.events.slice(attempt.offset).some(e=>e.type==='chat_setup_required'),false);
      }
    }
  }finally{await f.close();}
  const auth=await providerServer({auth:true});
  try {
    for(const endpoint of ['/api/chat/models?provider=codex','/api/cli','/api/cli/codex/status'])assert.equal((await fetch(auth.origin+endpoint)).status,401);
    const register=await fetch(auth.origin+'/api/auth/register',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({username:'fixture-user',password:'fixture-password'})});
    assert.equal(register.status,200);const {token}=await register.json();
    assert.equal((await auth.models('codex','',token)).problem.code,'provider_cli_unavailable');
    const second=await fetch(auth.origin+'/api/auth/register',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({username:'second-user',password:'fixture-password'})});
    assert.equal(second.status,403);assert.match(await second.text(),/single-user/);
    // There is no provider installer API, even for the authenticated owner.
    for(const endpoint of ['/api/cli/codex/install','/api/cli/claude/install','/api/cli/gemini/install']) {
      const r=await fetch(auth.origin+endpoint,{method:'POST',headers:{authorization:`Bearer ${token}`}});
      assert.ok([404,405].includes(r.status),`${endpoint}: ${r.status}`);
    }
    assert.deepEqual(await auth.records(),[]);
  }finally{await auth.close();}
});
