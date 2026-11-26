const assert = require('node:assert/strict');
const { test } = require('node:test');
const { openChatScrollPage } = require('./fixtures/chat-scroll-page.cjs');
test('slash commands accept keyboard and touch input above the composer without sending a model turn', { skip: !process.env.PLAYWRIGHT_MODULE }, async () => {
  const { chromium } = require(process.env.PLAYWRIGHT_MODULE);
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROME_EXECUTABLE ? { executablePath: process.env.CHROME_EXECUTABLE } : {}) });
  const newContext=browser.newContext.bind(browser);
  browser.newContext=options=>newContext({...options,hasTouch:options.viewport.width<500});
  try {
    for (const width of [1280,390]) {
      const fixture = await openChatScrollPage(browser,{width});
      const {page} = fixture;
      try {
        const commands=[{id:'app:help',name:'help',description:'Show supported commands',source:'workbench',available:true,verified:true},{id:'app:status',name:'status',description:'Show status',source:'workbench',available:true,verified:true}];
        let prepared=0,dialogs=0;
        await page.route('**/api/chat/commands?**',route=>route.fulfill({contentType:'application/json',body:JSON.stringify({commands,revision:'release-fixture'})}));
        await page.route('**/api/chat/commands/prepare',route=>{
          assert.equal(route.request().postDataJSON().prompt.trim(),'/help');
          prepared++;
          return route.fulfill({contentType:'application/json',body:JSON.stringify({action:'help'})});
        });
        page.on('dialog',async dialog=>{dialogs++;await dialog.accept();});
        const input=page.locator('#chat-prompt');
        await input.fill('/');
        const menu=page.locator('#chat-slash-commands');
        await menu.waitFor({state:'visible'});
        assert.equal(await menu.locator('[role=option]').count(),2);
        const bounds=await menu.boundingBox();
        assert.ok(bounds.x>=0 && bounds.x+bounds.width<=width+1);
        await input.fill('/he');
        if(width===1280)await input.press('Tab');
        else await menu.locator('[role=option]').first().tap();
        assert.equal(await input.inputValue(),'/help ');
        await input.press('Enter');
        await page.waitForFunction(()=>document.querySelector('#chat-prompt').value==='');
        assert.equal(prepared,1);
        assert.equal(dialogs,1);
        assert.equal(fixture.clientMessages.filter(m=>m.type==='start_session').length,0);
        assert.deepEqual(fixture.errors,[]);
      }finally{await fixture.close();}
    }
  }finally{await browser.close();}
});
