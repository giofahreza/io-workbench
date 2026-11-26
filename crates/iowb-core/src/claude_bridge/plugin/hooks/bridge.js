// No prompts are written into transcript files. Claude remains the only owner.
let connection = null;
let generation = 0;
let busy = false;
let stopped = false;
let submitting = null;
let awaitingRequest = null;
let nextRequest = null;
let active = null;
let events = [];
let exchanging = false;
let launching = false;
let sequence = 0;
let endedId = null;
let seen = new Set();
let cancelled = new Set();
let streamed = 0;
let activityCount = 0;

function boundedText(text,limit) {
  let value=text.slice(0,limit);
  const last=value.charCodeAt(value.length-1);
  if (last >= 0xd800 && last <= 0xdbff) value=value.slice(0,-1);
  return value;
}

function record(event) {
  events.push({...event,sequence:++sequence});
}

async function dispatch($, command) {
  if (command.kind === 'abort') {
    cancelled.add(command.id);
    if (active?.id === command.id) {
      await $.turn.abort({ turnId: active.turnId });
    } else if (submitting?.id !== command.id && awaitingRequest?.id !== command.id) {
      record({kind:'cancelled',requestId:command.id});
    }
    return;
  }
  if (command.kind !== 'submit') return;
  const request = command.request;
  if (seen.has(request.id)) return;
  seen.add(request.id);
  record({kind:'dispatch',requestId:request.id});
  if (cancelled.has(request.id)) {
    record({kind:'cancelled',requestId:request.id});
    return;
  }
  if (submitting || busy || stopped || (await $.session.id()) !== connection?.sessionId) {
    record({kind:'reject',requestId:request.id,error:'Claude is busy, stopped, or changed conversations; no message was sent'});
    return;
  }
  submitting = request;
  awaitingRequest = request;
  try {
    if (/^\s*[!/]/.test(request.prompt)) {
      // Built-ins with dialogs and identity changes stay in the native terminal.
      throw new Error('Run terminal commands in the attached Claude CLI. This message was not sent.');
    }
    const result = await $.prompt.submit({text:request.prompt,asUser:true});
    if (result?.drop) {
      awaitingRequest=null;
      record({kind:'reject',requestId:request.id,error:result.drop});
    }
  } catch (error) {
    awaitingRequest=null;
    record({kind:'reject',requestId:request.id,error:String(error?.message ?? error)});
  } finally {
    submitting = null;
  }
}

async function exchange($) {
  if (exchanging || !connection) return;
  exchanging = true;
  const current = connection;
  const batch = events.slice(0,256);
  try {
    const id = await $.session.id();
    const response = await $.http.fetch(current.url + '/exchange', {
      method:'POST',headers:{Authorization:'Bearer '+current.token,'Content-Type':'application/json'},
      body:JSON.stringify({sessionId:current.sessionId,instance:current.instance,connected:id===current.sessionId,busy,stopped,events:batch}),
    });
    if (!response.ok) throw new Error('bridge refused exchange');
    const reply = JSON.parse(response.text);
    if (connection !== current) return;
    events = events.filter(event => event.sequence > reply.ack);
    const command = reply.command;
    if (command && connection === current) void dispatch($,command).catch(error => $.ui.log(String(error),{to:'debug'}));
  } catch (error) {
    // Keep unacknowledged events. The helper deduplicates their sequences;
    // dispatch also deduplicates commands before calling prompt.submit.
  } finally { exchanging = false; }
}

async function launch($) {
  if (launching) return;
  launching = true;
  const epoch = ++generation;
  events=[]; sequence=0; seen=new Set(); cancelled=new Set();
  try {
    const id = await $.session.id();
    const cwd = await $.session.cwd();
    const binary = (await $.env.get('IO_WORKBENCH_CLAUDE_BRIDGE_BIN')) || 'io-workbench';
    const child = $.process.spawn({argv:[binary,'claude-bridge','--session-id',id,'--cwd',cwd]});
    let pending = '';
    for await (const chunk of child) {
      if (epoch !== generation) break;
      if (chunk.stream !== 'stdout') continue;
      pending += chunk.text;
      for (;;) {
        const end = pending.indexOf('\n');
        if (end < 0) break;
        const line = pending.slice(0,end); pending = pending.slice(end+1);
        if (!line) continue;
        const manifest = JSON.parse(line);
        if (manifest.protocol !== 1 || manifest.sessionId !== id) throw new Error('Invalid Workbench bridge handshake');
        connection = manifest;
        await exchange($);
      }
    }
  } catch (error) { $.ui.log('Workbench bridge: '+String(error?.message ?? error),{to:'debug'}); }
  finally { if (epoch === generation) connection = null; launching = false; }
}

export function register(on) {
  on('session.start',async ($,e,next) => {
    const result = await next(e);
    void launch($);
    $.clock.every(250,async () => {
      if (connection && (await $.session.id()) !== connection.sessionId) {
        await exchange($); generation++; connection=null;
        busy=false; stopped=false; active=null; nextRequest=null; submitting=null; awaitingRequest=null;
        events=[]; sequence=0; seen=new Set(); cancelled=new Set();
      }
      if (!launching && !connection && (await $.session.id()) !== endedId) void launch($);
      await exchange($);
    });
    return result;
  });
  on('prompt.submit',async ($,e,next) => {
    if (e.origin.kind === 'composer' || e.origin.kind === 'sdk' || e.origin.kind === 'bridge') stopped=false;
    return next(e);
  });
  on('session.append',async ($,e,next) => {
    if (e.door === 'prompt' && e.agentId === undefined) {
      // Own prompt.submit hooks are skipped by Claude's re-entry guard.
      // The engine's appended row carries the immutable origin and UUID;
      // correlate here, never by matching the human's prompt text.
      if (e.origin.kind === 'plugin' && e.origin.name === 'io-workbench-bridge') {
        nextRequest=awaitingRequest ? {...awaitingRequest,userMessageId:e.uuid} : null;
        awaitingRequest=null;
      } else {
        nextRequest=null;
      }
    }
    return next(e);
  });
  on('turn.start',async ($,e,next) => {
    if (e.agentId === undefined) {
      busy=true;
      streamed=0; activityCount=0;
      active = nextRequest ? {...nextRequest,turnId:e.turnId} : null;
      nextRequest=null;
      if (active) record({kind:'start',requestId:active.id,turnId:e.turnId,userMessageId:active.userMessageId});
      await exchange($);
      if (active && cancelled.has(active.id)) await $.turn.abort({turnId:e.turnId});
    }
    return next(e);
  });
  on('turn.step',async function* ($,e,next) {
    let input=e;
    const owned = e.agentId === undefined && active?.turnId === e.turnId ? active : null;
    if (owned?.model) input={...input,model:owned.model};
    if (owned?.effort) input={...input,effort:owned.effort};
    const stream=next(input);
    for (;;) {
      const part=await stream.next();
      if (part.done) return part.value;
      if (owned && part.value.kind === 'text' && streamed < 65536) {
        const text=boundedText(part.value.text,65536-streamed);
        streamed+=text.length;
        record({kind:'text',requestId:owned.id,text});
      }
      yield part.value;
    }
  });
  on('tool.call',async ($,e,next) => {
    if (e.agentId === undefined && active && activityCount++ < 96) record({kind:'activity',requestId:active.id,text:('[Tool: '+e.tool+']\n').slice(0,512)});
    return next(e);
  });
  on('turn.complete',async ($,e,next) => {
    if (e.agentId === undefined) {
      if (active?.turnId === e.turnId) record({kind:'complete',requestId:active.id,answer:boundedText(e.answer,65536),usage:e.usage,reason:e.reason,isAborted:e.isAborted});
      active=null; busy=false;
      await exchange($);
    }
    return next(e);
  });
  on('session.stop',async ($,e,next) => { stopped=true; await exchange($); return next(e); });
  on('session.end',async ($,e,next) => {
    if (active) record({kind:'uncertain',requestId:active.id,error:'The terminal changed or ended its conversation. Inspect its history before retrying.'});
    await exchange($);
    endedId = await $.session.id();
    generation++; connection=null;
    busy=false; stopped=false; active=null; nextRequest=null; submitting=null; awaitingRequest=null;
    events=[]; sequence=0; seen=new Set(); cancelled=new Set();
    return next(e);
  });
}
