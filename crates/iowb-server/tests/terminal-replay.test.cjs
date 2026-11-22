const assert = require('node:assert/strict');
const {test} = require('node:test');
const {providerServer, until, pause} = require('./fixtures/provider-server.cjs');

test('terminal replay preserves live chunk identities across late subscriptions and reconnects',
  {skip: !process.env.IOWB_TEST_BINARY, timeout: 30000}, async () => {
    const f = await providerServer();
    let process;
    try {
      const response = await fetch(f.origin+'/api/process', {method:'POST',headers:{'content-type':'application/json'},
        body:JSON.stringify({command:'/bin/sh',args:['-c',"printf 'EARLY_PROMPT\\n'; read first; printf 'LIVE_OUTPUT\\n'; read second"],pty:true,cwd:f.project})});
      assert.equal(response.status,200); process=await response.json();
      await pause(200); // Initial output exists before the socket knows the id.
      const ws=await f.websocket();
      const subscribe=socket=>socket.send(JSON.stringify({type:'subscribe',sessionIds:[],chatSessionIds:[],processIds:[process.id]}));
      subscribe(ws.socket);
      const initial=await until(()=>ws.events.find(e=>e.type==='process_output'&&e.data.includes('EARLY_PROMPT')));
      assert.equal(initial.processId,process.id);assert.ok(initial.sequence>0);
      ws.socket.send(JSON.stringify({type:'process_input',processId:process.id,data:'\n'}));
      const live=await until(()=>ws.events.find(e=>e.type==='process_output'&&e.data.includes('LIVE_OUTPUT')));
      assert.ok(live.sequence>initial.sequence);
      const offset=ws.events.length;subscribe(ws.socket);
      await until(()=>ws.events.slice(offset).find(e=>e.type==='process_output'&&e.data.includes('LIVE_OUTPUT')));
      const replay=ws.events.slice(offset).filter(e=>e.type==='process_output');
      assert.equal(replay.find(e=>e.data.includes('EARLY_PROMPT')).sequence,initial.sequence);
      assert.equal(replay.find(e=>e.data.includes('LIVE_OUTPUT')).sequence,live.sequence);
      const replacement=await f.websocket();subscribe(replacement.socket);
      const reopened=await until(()=>replacement.events.find(e=>e.type==='process_output'&&e.data.includes('EARLY_PROMPT')));
      assert.equal(reopened.sequence,initial.sequence,'reconnect must reuse the stored identity');
      // Replaying the same id twice must not duplicate chunks within one snapshot.
      const seqs=replay.map(e=>e.sequence);assert.equal(new Set(seqs).size,seqs.length);
    } finally {
      if(process)await fetch(f.origin+'/api/process/'+process.id,{method:'DELETE'});
      await f.close();
    }
  });
