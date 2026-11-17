const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { test } = require('node:test');
const vm = require('node:vm');

test('header expands only matching automatic titles from the latest user prompt', () => {
  const fullTitle = 'always restart this app that run in this machine on port 8100 if you make any changes in backend or web ui';
  const session = { title: `${fullTitle.slice(0, 96)}...`, titleSource: 'prompt' };
  const context = vm.createContext({
    state: { chatSessionId: 'session' },
    findChatSession: () => session,
    cachedChatSession: () => null,
    chatHistoryWindow: { sessionId: 'session', messages: [{ role: 'user', content: fullTitle }] },
  });
  vm.runInContext(readFileSync(join(__dirname, '../static/app/sidebar.js'), 'utf8'), context);
  context.findChatSession = () => session;
  assert.equal(vm.runInContext('workspaceViewTitle("chat")', context), fullTitle);
  session.titleSource = 'manual';
  assert.equal(vm.runInContext('workspaceViewTitle("chat")', context), session.title);
  session.titleSource = 'prompt';
  session.title = 'Different prompt...';
  assert.equal(vm.runInContext('workspaceViewTitle("chat")', context), session.title);
});

test('header uses session-scoped prompt history when the transcript omits or trims the prompt', () => {
  const fullTitle = 'Now help me to: - flip step icon vertically - Make temperature icon to use correct icon same as current weather (maybe like cloud / cloud with sun / moon / wind / rain / heavy rain / light rain / run cloudy / etc)';
  const session = { title: `${fullTitle.slice(0, 96)}...`, titleSource: 'prompt' };
  const context = vm.createContext({
    state: { chatSessionId: 'garmin', chatPromptHistoryScope: 'server::garmin', chatPromptHistory: [{ content: fullTitle }] },
    cachedChatSession: () => null,
    chatPromptHistoryScope: (id) => `server::${id}`,
    chatHistoryWindow: { sessionId: 'garmin', messages: [] },
  });
  vm.runInContext(readFileSync(join(__dirname, '../static/app/sidebar.js'), 'utf8'), context);
  context.findChatSession = () => session;
  assert.equal(vm.runInContext('workspaceViewTitle("chat")', context), fullTitle);
  context.chatHistoryWindow.messages = [{ role: 'user', content: '[chat message truncated]' }];
  assert.equal(vm.runInContext('workspaceViewTitle("chat")', context), fullTitle);
  context.state.chatPromptHistoryScope = 'another-session';
  assert.equal(vm.runInContext('workspaceViewTitle("chat")', context), session.title);
  context.state.chatPromptHistoryScope = 'server::garmin';
  session.titleSource = 'manual';
  assert.equal(vm.runInContext('workspaceViewTitle("chat")', context), session.title);
  session.titleSource = 'prompt';
  context.state.chatPromptHistory[0].content += '\n![screen](data:image/png;base64,QUJDRA==)';
  assert.equal(vm.runInContext('workspaceViewTitle("chat")', context), `${fullTitle} Attached image: screen`);
});

test('prompt history completion refreshes the header and ignores a switched session', async () => {
  let refreshes = 0;
  let finishRequest;
  const context = vm.createContext({
    console, URLSearchParams, PROMPT_HISTORY_PAGE_SIZE: 30,
    state: { chatSessionId: 'garmin', chatPromptHistoryScope: 'garmin', chatPromptHistory: [] },
    findChatSession: () => ({ id: 'garmin' }),
    canLoadProtectedData: () => true,
    api: () => new Promise((resolve) => { finishRequest = resolve; }),
    updateMainHeader: () => { refreshes += 1; },
  });
  vm.runInContext(readFileSync(join(__dirname, '../static/app/chat/prompt-history.js'), 'utf8'), context);
  context.ensureChatPromptHistoryScope = () => {};
  context.chatPromptHistoryScope = (id) => id;
  context.mergeChatPromptHistory = (prompts) => { context.state.chatPromptHistory = prompts; };
  const first = vm.runInContext('loadChatPromptHistoryPage("garmin")', context);
  finishRequest({ session_id: 'garmin', prompts: [{ content: 'Full prompt' }] });
  assert.equal(await first, true);
  assert.equal(refreshes, 1);
  const second = vm.runInContext('loadChatPromptHistoryPage("garmin")', context);
  context.state.chatPromptHistoryScope = 'other-session';
  finishRequest({ session_id: 'garmin', prompts: [{ content: 'Stale prompt' }] });
  assert.equal(await second, false);
  assert.equal(refreshes, 1);
});

test('chat title fills the space beside navigation in one compact row', {
  skip: !process.env.PLAYWRIGHT_MODULE,
}, async () => {
  const { chromium } = require(process.env.PLAYWRIGHT_MODULE);
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_EXECUTABLE ? { executablePath: process.env.CHROME_EXECUTABLE } : {}),
  });
  const root = join(__dirname, '../static');
  const header = readFileSync(join(root, 'index.html'), 'utf8')
    .match(/<header class="main-content-header">[\s\S]*?<\/header>/)[0];
  try {
    for (const width of [390, 760, 761, 900, 1280, 1920]) {
      const page = await browser.newPage({ viewport: { width, height: 800 } });
      await page.route('http://title.test/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        if (path.endsWith('.css')) {
          return route.fulfill({ contentType: 'text/css', body: readFileSync(join(root, path), 'utf8') });
        }
        return route.fulfill({ contentType: 'text/html', body:
          `<link rel="stylesheet" href="/styles.css"><aside class="sidebar"></aside><main class="workspace">${header}<section></section></main>` });
      });
      await page.goto('http://title.test/');
      for (const text of ['New chat', 'Continue implementing CRM missing features and verify the complete user-facing workflow', 'LongTitle'.repeat(100)]) {
        const result = await page.evaluate((text) => {
          const title = document.querySelector('#view-title');
          title.textContent = text;
          const header = document.querySelector('.main-content-header');
          const nav = document.querySelector('.main-tab-switcher');
          const subtitle = document.querySelector('#view-subtitle');
          subtitle.textContent = 'io-workbench';
          const h = header.getBoundingClientRect();
          const t = title.getBoundingClientRect();
          const n = nav.getBoundingClientRect();
          const project = subtitle.getBoundingClientRect();
          const style = getComputedStyle(header);
          return {
            hidden: style.display === 'none',
            available: n.left - t.left - parseFloat(style.columnGap),
            width: t.width, height: t.height, lineHeight: parseFloat(getComputedStyle(title).lineHeight),
            textFits: title.scrollWidth <= title.clientWidth,
            sameRow: n.top < project.bottom && n.bottom > t.top, navFits: n.right <= h.right,
            projectVisible: getComputedStyle(subtitle).display !== 'none' && project.height > 0,
            projectBelowTitle: project.top >= t.bottom && project.top - t.bottom <= 2,
            headerHeight: h.height,
            overflow: document.documentElement.scrollWidth > innerWidth,
          };
        }, text);
        assert.equal(result.hidden, width <= 760, `visibility at ${width}`);
        if (width > 760) {
          assert.ok(Math.abs(result.width - result.available) < 1, `full title width at ${width}: ${JSON.stringify(result)}`);
          assert.ok(Math.abs(result.height - result.lineHeight) < 1, `single row at ${width}`);
          if (width >= 1280 && !text.startsWith('LongTitle')) {
            assert.equal(result.textFits, true, `complete title at ${width}`);
          }
          assert.ok(result.sameRow && result.navFits, `navigation at ${width}`);
          assert.ok(result.projectVisible && result.projectBelowTitle, `project folder below title at ${width}`);
          assert.ok(result.headerHeight < 55, `compact header at ${width}`);
          assert.equal(result.overflow, false, `page overflow at ${width}`);
        }
      }
      await page.close();
    }
  } finally { await browser.close(); }
});
