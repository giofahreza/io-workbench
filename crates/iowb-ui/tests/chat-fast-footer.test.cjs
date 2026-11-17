const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');
const { test } = require('node:test');

function fixture() {
  const context = vm.createContext({ console });
  for (const file of ['history', 'stream', 'settings']) {
    vm.runInContext(readFileSync(join(__dirname, `../static/app/chat/${file}.js`), 'utf8'), context);
  }
  vm.runInContext(`
    const state = { preferences: { chatFast: true }, chatSessionId: 'session' };
  `, context);
  return code => vm.runInContext(code, context);
}

test('footer places fast immediately after permission mode, only when enabled', () => {
  const run = fixture();
  assert.equal(run(`chatLineFooterSummary({model:'model', mode:'bypass', fast:true, elapsed:'2s'})`), 'model · bypass · fast · 2s');
  assert.equal(run(`chatLineFooterSummary({model:'model', mode:'plan', fast:true})`), 'model · plan · fast');
  for (const value of ['false', 'null', 'undefined']) {
    assert.equal(run(`chatLineFooterSummary({model:'model', mode:'bypass', fast:${value}})`), 'model · bypass');
  }
  assert.equal(run(`normalizedChatMessageMeta({fast:'false'}).fast`), false);
});

test('history retains each turn setting and does not infer fast from current controls', () => {
  const run = fixture();
  for (const role of ['user', 'assistant']) {
    assert.equal(run(`chatMessageFooterMeta({metadata:{fast:false}}, '${role}', {fast:true}).fast`), false);
    assert.equal(run(`chatMessageFooterMeta({metadata:{fast:true}}, '${role}', {fast:false}).fast`), true);
    assert.equal(run(`chatMessageFooterMeta({}, '${role}', {fast:true}).fast`), undefined);
  }
  assert.equal(run(`chatMessageFooterMeta({}, 'assistant', {fast:false}, {metadata:{fast:true}}).fast`), true);
  assert.equal(run(`chatMessageFooterMeta({metadata:{fast:false}}, 'assistant', {}, {metadata:{fast:true}}).fast`), false);
});

test('live response reads its submitted prompt, not mutable preferences or another session', () => {
  const run = fixture();
  run(`chatHistoryWindow = {sessionId:'session', messages:[{role:'user', metadata:{fast:false}}]}`);
  assert.equal(run(`chatTurnFast('session')`), false);
  assert.equal(run(`chatTurnFast('other')`), undefined);
  run(`chatHistoryWindow.messages.push({role:'user', metadata:{fast:true}}); state.preferences.chatFast = false`);
  assert.equal(run(`chatTurnFast('session')`), true);
});
