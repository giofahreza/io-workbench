const assert = require('node:assert/strict');
const { test } = require('node:test');
const { openChatScrollPage } = require('./fixtures/chat-scroll-page.cjs');

const enabled={skip:!process.env.PLAYWRIGHT_MODULE,timeout:120000};
const docs={codex:'https://developers.openai.com/codex/cli/',claude:'https://code.claude.com/docs/en/setup',gemini:'https://geminicli.com/docs/get-started/installation/'};
async function browserTest(callback){
  const {chromium}=require(process.env.PLAYWRIGHT_MODULE);
  const browser=await chromium.launch({headless:true,...(process.env.CHROME_EXECUTABLE?{executablePath:process.env.CHROME_EXECUTABLE}:{})});
  try{await callback(browser);}finally{await browser.close();}
}
async function provider(page,p){
  await page.evaluate(p=>{state.preferences.chatCli=p;state.preferences.chatProvider=p;},p);
}

test('PR-001..007/025/026/028/029 MC-002..008 ER-006/014/015: setup/recheck/link and provider independence in desktop/mobile browsers',enabled,async()=>browserTest(async browser=>{
  for(const width of [1280,390]) {
    const f=await openChatScrollPage(browser,{width});const {page}=f;
    let installed=false,readyProvider='',failNetwork=false,mode='missing',suffix='';
    const queries=[];
    await page.route('**/api/chat/models?**',route=>{
      const url=new URL(route.request().url());const p=url.searchParams.get('provider');queries.push(url.search);
      if(failNetwork)return route.abort();
      const ready=mode==='ready'||p===readyProvider;
      const runtime=url.searchParams.get('runtime')||'native_cli';
      const problems={missing:{code:'provider_cli_unavailable',message:`${p} CLI is missing on the Workbench server.`,action:'install',docsUrl:docs[p]+suffix},
        gateway:{code:'gateway_auth_required',message:'Gateway key rejected.',action:'gateway_settings'},
        empty:{code:'gateway_catalog_empty',message:'IO Gateway returned no models.',action:'gateway_settings'},
        malformed:{code:'gateway_catalog_unavailable',message:'Could not read gateway catalog.',action:'gateway_settings'}};
      return route.fulfill({contentType:'application/json',body:JSON.stringify({provider:p,runtime,ready,models:ready?[{value:runtime==='io_gateway'?'cod:gpt-test':'',label:runtime==='io_gateway'?'cod:gpt-test':'Native CLI default'}]:[],
        readiness:{installed:installed||ready,nodeAvailable:false,npmAvailable:false,nodeInstallUrl:'https://nodejs.org/en/download',authState:p==='gemini'&&ready?'configured_unverified':'required'},problem:ready?null:problems[mode]})});
    });
    // Official navigation is intercepted, including popups; no documentation
    // request leaves the browser during this acceptance run.
    await f.context.route('**/*',route=>{
      if(new URL(route.request().url()).origin==='http://chat.test')return route.fallback();
      return route.fulfill({status:503,body:'Offline documentation fixture'});
    });
    try {
      for(const p of Object.keys(docs)) {
        await provider(page,p);await page.evaluate(p=>loadChatModelsIntoSelect(p,true),p);
        await page.evaluate(()=>togglePromptConfigPanel(true));
        const card=page.locator('#chat-provider-readiness');
        assert.match(await card.innerText(),new RegExp(`${p}.*server`));
        assert.match(await card.innerText(),/not on your phone/);
        const guide=card.getByText('Official setup guide');
        assert.equal(await guide.getAttribute('href'),docs[p]);
        const rows=await page.locator('#chat-output .chat-line').count();
        await card.getByRole('button',{name:'Recheck',exact:true}).click();
        await page.waitForFunction(()=>document.querySelector('#chat-model').disabled);
        assert.match(await card.innerText(),/missing/);
        assert.equal(await page.locator('#chat-output .chat-line').count(),rows);
        assert.match(queries.at(-1),/refresh=true/);
        const popup=page.waitForEvent('popup');await guide.click();const opened=await popup;
        await opened.waitForLoadState();assert.equal(opened.url(),docs[p]);await opened.close();
        suffix='?source=workbench#install';
        await card.getByRole('button',{name:'Recheck',exact:true}).click();
        await page.waitForFunction(()=>document.querySelector('#chat-provider-readiness a')?.href.includes('source=workbench'));
        assert.equal(await card.getByText('Official setup guide').getAttribute('href'),docs[p]+suffix);
        const updatedPopup=page.waitForEvent('popup');await card.getByText('Official setup guide').click();
        const updated=await updatedPopup;await updated.waitForLoadState();assert.equal(updated.url(),docs[p]+suffix);await updated.close();
        suffix='';mode='ready';installed=true;
        await card.getByRole('button',{name:'Recheck',exact:true}).click();
        await page.waitForFunction(()=>!document.querySelector('#chat-model').disabled);
        assert.equal(await card.isVisible(),false);
        if(p==='gemini')assert.match(await page.locator('#chat-route-status').innerText(),/authentication unverified/);
        mode='missing';installed=false;
      }
      for(readyProvider of ['codex','claude'])for(const p of Object.keys(docs)) {
        await provider(page,p);await page.evaluate(p=>loadChatModelsIntoSelect(p,true),p);
        assert.equal(await page.locator('#chat-model').isDisabled(),p!==readyProvider);
        assert.equal(await page.locator('#chat-provider-readiness').isVisible(),p!==readyProvider);
      }
      readyProvider='';
      for(mode of ['gateway','empty','malformed']) {
        await provider(page,'codex');await page.evaluate(()=>{state.newChatRuntime='io_gateway';return loadChatModelsIntoSelect('codex',true);});
        assert.equal(await page.locator('#chat-model').isDisabled(),true);
        assert.equal(await page.locator('#chat-provider-readiness').getByText('Configure IO Gateway').count(),1);
        assert.doesNotMatch(await page.locator('#chat-model').innerText(),/CLI default/);
      }
      mode='missing';await page.evaluate(()=>loadChatModelsIntoSelect('codex',true));
      failNetwork=true;
      await page.locator('#chat-provider-readiness').getByRole('button',{name:'Recheck',exact:true}).click();
      await page.waitForFunction(()=>document.querySelector('#chat-provider-readiness').innerText.includes('connection'));
      assert.equal(await page.evaluate(()=>state.newChatRuntime),'io_gateway');
      assert.equal(await page.locator('#chat-model').isDisabled(),true);
      assert.equal(await page.locator('#chat-provider-readiness').getByText('Official setup guide').getAttribute('href'),docs.codex);
      failNetwork=false;
      await page.evaluate(()=>renderChatReadiness({runtime:'native_cli',ready:false,problem:{message:'Install manually on the Workbench server.',docsUrl:'javascript:alert(1)'}}));
      assert.equal(await page.locator('#chat-provider-readiness a').count(),0);
      assert.match(await page.locator('#chat-provider-readiness').innerText(),/Install manually/);
      assert.equal(f.requests.some(r=>/install|login/.test(r.path)),false);
      assert.deepEqual(f.errors,[]);
    } finally{await f.close();}
  }
}));

test('RT-001/002 ER-009: settings note, save and refresh never persist gateway secrets in browser storage or URLs',enabled,async()=>browserTest(async browser=>{
  const f=await openChatScrollPage(browser);const {page}=f;const secret='fixture-browser-key';let config={mode:'off',chatRuntime:'native_cli'};const writes=[];
  await page.route('**/api/settings/direct-ai',async route=>{
    if(route.request().method()==='PUT') {const body=route.request().postDataJSON();writes.push(body);config={...body,gatewayApiKey:undefined,gatewayApiKeyConfigured:true};}
    return route.fulfill({contentType:'application/json',body:JSON.stringify({config,apiKeyConfigured:!!config.gatewayApiKeyConfigured,effective:{chatRuntime:config.chatRuntime}})});
  });
  try {
    await page.evaluate(()=>{switchView('settings');setSettingsTab('direct-ai');return loadIoGatewayConfig({force:true});});
    assert.equal(await page.locator('#io-gateway-enabled').isChecked(),false);
    assert.equal(await page.locator('#io-gateway-route-note').isVisible(),false);
    await page.locator('#io-gateway-enabled').check();
    assert.match(await page.locator('#io-gateway-route-note').innerText(),/new chats.*Existing chats keep their original route/);
    await page.locator('#io-gateway-url').fill('https://gateway.example.test');
    await page.locator('#io-gateway-api-key').fill(secret);
    await page.locator('#save-direct-ai').click();
    await page.waitForFunction(()=>document.querySelector('#io-gateway-api-key').value==='');
    assert.equal(writes.length,1);assert.equal(writes[0].gatewayApiKey,secret);
    const storage=await page.evaluate(()=>JSON.stringify({local:{...localStorage},session:{...sessionStorage},url:location.href}));
    assert.ok(!storage.includes(secret));
    assert.equal(f.requests.some(r=>(r.path+r.query).includes(secret)),false);
    await page.reload({waitUntil:'networkidle'});
    await page.evaluate(()=>{switchView('settings');setSettingsTab('direct-ai');return loadIoGatewayConfig({force:true});});
    assert.equal(await page.locator('#io-gateway-api-key').inputValue(),'');
    assert.equal(await page.locator('#io-gateway-route-note').isVisible(),true);
    await page.locator('#io-gateway-enabled').uncheck();
    assert.equal(await page.locator('#io-gateway-route-note').isVisible(),false);
    assert.deepEqual(f.errors,[]);
  }finally{await f.close();}
}));

test('RT-016..022 ER-001/003/004: actual WebSocket dispatch, old connections, replay and generic errors',enabled,async()=>browserTest(async browser=>{
  const f=await openChatScrollPage(browser);const {page}=f;
  try {
    await f.stopPolling();
    await page.evaluate(()=>{state.chatSetupAttempt={sessionId:state.chatSessionId,requestId:'active-request',handled:false,draft:'Keep draft'};window.__oldSetupSocket=state.ws;connectWs();});
    await page.waitForFunction(()=>state.ws?.readyState===WebSocket.OPEN);
    assert.equal(await page.evaluate(()=>state.ws===window.__oldSetupSocket),false,'a replacement connection is established');
    await page.evaluate(()=>window.__oldSetupSocket.dispatchEvent(new MessageEvent('message',{data:JSON.stringify({type:'chat_setup_required',sessionId:state.chatSessionId,requestId:'active-request',code:'chat_runtime_mismatch',message:'OLD CONNECTION',action:'new_chat'})})));
    assert.equal(await page.locator('#chat-output [data-chat-history-key="durable-scoped-mismatch"]').count(),0);
    const dispatch=async payload=>page.evaluate(payload=>state.ws.dispatchEvent(new MessageEvent('message',{data:JSON.stringify(payload)})),payload);
    const base={type:'chat_setup_required',sessionId:f.session.id,requestId:'active-request',code:'chat_runtime_mismatch',message:'Correct scoped mismatch',action:'new_chat',
      historyMessage:{id:'durable-scoped-mismatch',role:'system',content:'Correct scoped mismatch',timestamp:'2026-09-30T01:01:00Z',metadata:{kind:'chat_error',phase:'rejected',code:'chat_runtime_mismatch',action:'new_chat'}}};
    for(const override of [{requestId:'old-request'},{requestId:null},{sessionId:'other-session'}])await dispatch({...base,...override});
    assert.equal(await page.locator('#chat-output [data-chat-history-key="durable-scoped-mismatch"]').count(),0);
    for(const message of ['model not supported when using an account','model is not supported when using Codex with a ChatGPT account']) {
      await dispatch({type:'error',sessionId:f.session.id,message});
      assert.equal(await page.locator('#chat-output [data-chat-history-key="durable-scoped-mismatch"]').count(),0);
      assert.ok((await page.locator('#chat-output').innerText()).includes(message));
    }
    const before=await page.locator('#chat-output').innerText();
    await dispatch({type:'error',message:'System-only connection failure'});
    assert.equal(await page.locator('#chat-output').innerText(),before,'sessionless errors must not be appended to the selected provider chat');
    await dispatch(base);await dispatch(base);
    assert.equal(await page.locator('#chat-output [data-chat-history-key="durable-scoped-mismatch"]').count(),1);
    await page.evaluate(()=>{window.__oldSetupSocket=state.ws;connectWs();});
    await page.waitForFunction(()=>state.ws?.readyState===WebSocket.OPEN);
    await dispatch(base);
    assert.equal(await page.locator('#chat-output [data-chat-history-key="durable-scoped-mismatch"]').count(),1);
    assert.deepEqual(f.errors,[]);
  }finally{await f.close();}
}));
