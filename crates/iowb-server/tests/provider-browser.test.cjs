const assert = require('node:assert/strict');
const { test } = require('node:test');
const { execFileSync } = require('node:child_process');
const { providerServer, until, pause } = require('./fixtures/provider-server.cjs');

test('RT-009/012/013/014/018/019 ER-002/009 PR-027/030/031: running app, recovery next-send, two tabs and reload',{
  skip:!process.env.IOWB_TEST_BINARY || !process.env.PLAYWRIGHT_MODULE,timeout:120000,
},async t=>{
  const f=await providerServer();
  const {chromium}=require(process.env.PLAYWRIGHT_MODULE);
  const browser=await chromium.launch({headless:true});
  const context=await browser.newContext({viewport:{width:1280,height:900},serviceWorkers:'block'});
  const errors=[],requests=[];
  await context.route('**/*',route=>{
    const url=new URL(route.request().url());requests.push({url:url.href,method:route.request().method()});
    return url.origin===f.origin?route.continue():route.abort();
  });
  const page=await context.newPage();page.setDefaultTimeout(15000);page.on('pageerror',e=>errors.push(e.message));
  await context.addInitScript(()=>{
    localStorage.setItem('iowb.lastView','chat');
    window.addEventListener('iowb:app-ready',()=>{window.__acceptanceReady=true;});
  });
  try {
    execFileSync('/usr/bin/git',['init',f.project],{env:{PATH:'/usr/bin:/bin',HOME:f.root,GIT_CONFIG_GLOBAL:'/dev/null',GIT_CONFIG_NOSYSTEM:'1'},stdio:'ignore'});
    const created=await fetch(f.origin+'/api/projects/create',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({path:f.project})});
    assert.equal(created.status,200);
    await page.goto(f.origin,{waitUntil:'networkidle'});
    await page.waitForFunction(()=>window.__acceptanceReady && state.ws?.readyState===WebSocket.OPEN);
    await page.evaluate(project=>startNewChatForProject(project),f.project);
    await t.test('no providers or credentials: files, Git, database, terminal and settings still open',async()=>{
      for(const view of ['files','git','database','shell','settings','chat']) {
        await page.evaluate(view=>switchView(view),view);
        assert.equal(await page.locator(`#${view}-view`).isVisible(),true);
      }
      await page.evaluate(()=>loadChatModelsIntoSelect(chatCliValue(),true));
      assert.equal(await page.locator('#chat-provider-readiness').isVisible(),true);
      assert.match(await page.locator('#chat-provider-readiness').innerText(),/server/);
    });
    for(const provider of ['codex','claude'])await t.test(`${provider}: install, native chat, mismatch, new gateway chat and reopen`,async()=>{
      await f.configure('native_cli',{mode:'off'});
      await f.install(provider);await pause(1050);
      await page.evaluate(async({project,provider})=>{
        state.preferences.chatModel='cod:stale-gateway-model';savePreferences();
        await startNewChatForProject(project);chooseNewChatProvider(provider);await loadChatModelsIntoSelect(provider,true);
      },{project:f.project,provider});
      assert.equal(await page.locator('#chat-model').inputValue(),'','stale gateway model removed before native send');
      await page.locator('#chat-prompt').fill(`Native ${provider} prompt`);
      await page.locator('#chat-submit').click();
      const nativeId=await until(()=>page.evaluate(()=>state.chatSessionId||false),'native chat id');
      await until(async()=>{
        const snapshot=await f.request(`/api/sessions/${nativeId}/snapshot?limit=10`);
        return !snapshot.session.active && snapshot.messages.some(m=>m.role==='assistant');
      },'native answer');
      const old=await f.request(`/api/sessions/${nativeId}/snapshot?limit=10`);
      assert.equal(old.session.runtime,'native_cli');
      await f.configure();
      await page.reload({waitUntil:'networkidle'});
      await page.waitForFunction(()=>window.__acceptanceReady && state.ws?.readyState===WebSocket.OPEN);
      await page.evaluate(({nativeId,project})=>pickChatSession(nativeId,project),{nativeId,project:f.project});
      await page.evaluate(()=>loadChatModelsIntoSelect(chatCliValue(),true));
      assert.match(await page.locator('#chat-route-status').innerText(),/Pinned: Native CLI/);
      const model=provider==='codex'?'cod:gpt-test':'cld:sonnet-test';
      // Simulate a stale saved selection from a legacy client. The current
      // catalog itself correctly does not offer gateway models in native chat.
      await page.evaluate(model=>{const select=document.querySelector('#chat-model');select.add(new Option(model,model));select.value=model;state.preferences.chatModel=model;},model);
      const before=(await f.records()).length;
      await page.locator('#chat-prompt').fill(`Recovered ${provider} draft`);
      await page.locator('#chat-submit').click();
      const card=page.locator('#chat-output .chat-setup-card');await card.waitFor();
      assert.equal((await f.records()).length,before);
      assert.equal(await page.locator('#chat-prompt').inputValue(),`Recovered ${provider} draft`);
      assert.match(await card.innerText(),/new chat/i);
      await card.getByRole('button',{name:'Start new chat'}).click();
      await page.waitForFunction(()=>state.newChatRuntime==='io_gateway' && state.chatRouteCatalog?.ready);
      assert.equal(await page.locator('#chat-prompt').inputValue(),`Recovered ${provider} draft`);
      assert.equal(await page.evaluate(()=>activeProjectPath()),f.project);
      await page.locator('#chat-submit').click();
      const gatewayId=await until(()=>page.evaluate(nativeId=>state.chatSessionId && state.chatSessionId!==nativeId && state.chatSessionId,nativeId),'new gateway chat');
      await until(async()=>{
        const response=await fetch(`${f.origin}/api/sessions/${gatewayId}/snapshot?limit=10`);
        if(response.status===404)return false;
        assert.equal(response.status,200);
        const snapshot=await response.json();
        return !snapshot.session.active && snapshot.messages.some(m=>m.role==='assistant');
      },'gateway answer');
      assert.equal((await f.request(`/api/sessions/${gatewayId}/snapshot?limit=10`)).session.runtime,'io_gateway');
      const reopened=await f.request(`/api/sessions/${nativeId}/snapshot?limit=10`);
      assert.deepEqual(reopened.messages,old.messages,'rejected send leaves persisted old transcript intact');
      await page.evaluate(({nativeId,project})=>pickChatSession(nativeId,project),{nativeId,project:f.project});
      await page.evaluate(()=>loadChatModelsIntoSelect(chatCliValue(),true));
      assert.match(await page.locator('#chat-output').innerText(),new RegExp(`Native ${provider} prompt`));
      assert.match(await page.locator('#chat-route-status').innerText(),/Pinned: Native CLI/);
      // A second real tab opens the same chat. A mismatch in one connection
      // must not introduce an error in the other tab's different attempt.
      const other=await context.newPage();other.on('pageerror',e=>errors.push(e.message));
      await other.goto(f.origin,{waitUntil:'networkidle'});
      await other.waitForFunction(()=>window.__acceptanceReady && state.ws?.readyState===WebSocket.OPEN);
      await other.evaluate(({nativeId,project})=>pickChatSession(nativeId,project),{nativeId,project:f.project});
      await page.evaluate(model=>{const select=document.querySelector('#chat-model');select.add(new Option(model,model));select.value=model;},model);
      await page.locator('#chat-prompt').fill('Mismatch while another tab is open');
      await page.locator('#chat-submit').click();
      await page.locator('#chat-output .chat-setup-card').waitFor();
      await pause(100);
      assert.equal(await other.locator('#chat-output .chat-setup-card').count(),0);
      await other.evaluate(project=>startNewChatForProject(project),f.project);
      await page.locator('#chat-prompt').fill('Another scoped mismatch');
      await page.locator('#chat-submit').click();
      await pause(100);
      assert.equal(await other.locator('#chat-output .chat-setup-card').count(),0);
      await other.close();
      await f.configure('native_cli',{mode:'off'});
    });
    assert.deepEqual(errors,[]);
    assert.equal(requests.some(r=>r.url.includes('fake-gateway-key')),false);
    assert.ok(!JSON.stringify(await page.evaluate(()=>({...localStorage}))).includes('fake-gateway-key'));
  }finally{await context.close();await browser.close();await f.close();}
});
