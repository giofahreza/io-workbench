const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { test } = require('node:test');
const vm = require('node:vm');

function fixture() {
  const context = vm.createContext({ URL, URLSearchParams, console });
  vm.runInContext(readFileSync(join(__dirname, '../static/app/chat/settings.js'), 'utf8'), context);
  vm.runInContext(`
    const state = { preferences: {}, chatSessionId: 'session-a', newChatRuntime: '', chatModelsGeneration: 0 };
    const select = { dataset: {}, disabled: false, innerHTML: '', value: '' };
    const requests = [];
    let provider = 'codex';
    qs = () => select;
    canLoadProtectedData = () => true;
    chatCliValue = () => provider;
    isActiveChatSessionEvent = (event) => event.sessionId === state.chatSessionId;
    api = (url) => new Promise((resolve, reject) => requests.push({ url, resolve, reject }));
    renderChatReadiness = (body) => { state.rendered = body; };
    escapeHtml = (text) => text.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('"', '&quot;');
    savePreferences = () => {};
  `, context);
  return context;
}
const run = (context, code) => vm.runInContext(code, context);

test('setup event requires exact session and request; duplicates and missing IDs are ignored', () => {
  const c = fixture();
  run(c, `state.chatSetupAttempt = { sessionId: 'session-a', requestId: 'send-2', handled: false };`);
  for (const [sessionId, requestId, expected] of [
    ['session-a', 'send-2', true], ['session-b', 'send-2', false],
    ['session-a', 'send-1', false], ['session-a', null, false], ['', 'send-2', false],
  ]) {
    assert.equal(run(c, `chatSetupEventMatches(${JSON.stringify({ sessionId, requestId })}, state.chatSetupAttempt)`), expected);
  }
  run(c, `state.chatSetupAttempt.handled = true;`);
  assert.equal(run(c, `chatSetupEventMatches({sessionId:'session-a', requestId:'send-2'}, state.chatSetupAttempt)`), false);
  run(c, `state.chatSetupAttempt.handled = false; state.chatSessionId = 'other';`);
  assert.equal(run(c, `chatSetupEventMatches({sessionId:'session-a', requestId:'send-2'}, state.chatSetupAttempt)`), false);
});

test('documentation links only allow official HTTPS hosts, including safe query/fragment data', () => {
  const c = fixture();
  for (const raw of ['javascript:alert(1)', 'https://evil.test/install', 'https://code.claude.com.evil.test/', 'https://user:secret@code.claude.com/', 'http://nodejs.org', null, 'not a URL']) {
    assert.equal(run(c, `safeProviderDocsUrl(${JSON.stringify(raw)})`), '');
  }
  const url = 'https://code.claude.com/docs/en/setup?source=workbench#install';
  assert.equal(run(c, `safeProviderDocsUrl(${JSON.stringify(url)})`), url);
});

test('late same-provider responses cannot replace a newer route catalog', async () => {
  const c = fixture();
  const first = run(c, `loadChatModelsIntoSelect('codex')`);
  run(c, `state.newChatRuntime = 'io_gateway'`);
  const second = run(c, `loadChatModelsIntoSelect('codex')`);
  run(c, `requests[1].resolve({runtime:'io_gateway', ready:true, models:[{value:'cod:gpt-test',label:'Gateway'}]})`);
  await second;
  run(c, `requests[0].resolve({runtime:'native_cli',ready:true,models:[{value:'native-test',label:'Native'}]})`);
  await first;
  assert.match(run(c, 'select.innerHTML'), /cod:gpt-test/);
  assert.doesNotMatch(run(c, 'select.innerHTML'), /native-test/);
  assert.equal(run(c, 'state.rendered.runtime'), 'io_gateway');
  assert.match(run(c, 'requests[1].url'), /sessionId=session-a/);
  assert.match(run(c, 'requests[1].url'), /runtime=io_gateway/);
});

test('provider switching and session navigation discard old catalog successes and failures', async () => {
  const c = fixture();
  const first = run(c, `loadChatModelsIntoSelect('codex')`);
  run(c, `provider='claude'`);
  const second = run(c, `loadChatModelsIntoSelect('claude')`);
  run(c, `requests[1].resolve({runtime:'native_cli',ready:true,models:[{value:'sonnet',label:'Sonnet'}]})`);
  await second;
  run(c, `requests[0].reject(new Error('old failure'))`);
  await first;
  assert.match(run(c, 'select.innerHTML'), /Sonnet/);
  const third = run(c, `loadChatModelsIntoSelect('claude')`);
  run(c, `state.chatSessionId='session-b'; requests[2].resolve({ready:true,models:[{value:'stale',label:'Stale'}]})`);
  await third;
  assert.doesNotMatch(run(c, 'select.innerHTML'), /Stale/);
});

test('failed or unavailable routes never manufacture a native CLI default', async () => {
  for (const failure of ['network', 'setup']) {
    const c = fixture();
    run(c, `state.preferences.chatModel = 'cod:old-model'`);
    const pending = run(c, `loadChatModelsIntoSelect('codex', true)`);
    run(c, failure === 'network' ? `requests[0].reject(new Error('offline'))` : `requests[0].resolve({ready:false,models:[],problem:{message:'Install the CLI'}})`);
    await pending;
    assert.doesNotMatch(run(c, 'select.innerHTML'), /CLI default/);
    assert.equal(run(c, 'select.disabled'), true);
    assert.equal(run(c, 'state.preferences.chatModel'), '');
    assert.match(run(c, 'requests[0].url'), /refresh=true/);
  }
});

test('stale model preferences are replaced by a model in the current catalog', async () => {
  const c = fixture();
  run(c, `state.preferences.chatModel='cod:old'`);
  const pending = run(c, `loadChatModelsIntoSelect('codex')`);
  run(c, `requests[0].resolve({ready:true,runtime:'io_gateway',models:[{value:'cod:new',label:'New'}]})`);
  await pending;
  assert.equal(run(c, 'state.preferences.chatModel'), 'cod:new');
});

test('offline Recheck retains documentation only for the same provider, session and route', async () => {
  for (const change of ['', "provider='claude'", "state.newChatRuntime='io_gateway'", "state.chatSessionId='session-b'"]) {
    const c=fixture();
    const initial=run(c,"loadChatModelsIntoSelect('codex',true)");
    run(c,`requests[0].resolve({provider:'codex',runtime:'native_cli',ready:false,models:[],problem:{message:'Install on server',docsUrl:'https://developers.openai.com/codex/cli/'}})`);
    await initial;
    if(change)run(c,change);
    const retry=run(c,'loadChatModelsIntoSelect(provider,true)');
    run(c,"requests[1].reject(new Error('offline'))");await retry;
    assert.equal(run(c,'state.rendered.ready'),false);
    assert.equal(run(c,'select.disabled'),true);
    assert.equal(run(c,'state.rendered.problem.docsUrl'),change?null:'https://developers.openai.com/codex/cli/');
  }
});
