#!/usr/bin/env node
// Full local acceptance run. Provider installation, login and paid requests
// are never part of this runner. Install test dependencies separately.
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';

const repository = resolve(dirname(fileURLToPath(import.meta.url)), '..');
if (!process.env.PLAYWRIGHT_MODULE) throw new Error('Set PLAYWRIGHT_MODULE to an installed Playwright package with Chromium. Browser tests must not be skipped.');
const parent = join(homedir(), '.cache', 'iowb-tests');
await mkdir(parent, { recursive: true });
const root = await mkdtemp(join(parent, 'isolated-suite-'));
const env = { ...process.env };
for (const key of Object.keys(env)) {
  if (/(?:TOKEN|API_KEY|SECRET|CREDENTIAL)/i.test(key)
    || /^(?:CODEX_HOME|CLAUDE_|ANTHROPIC_|OPENAI_|GEMINI_|GOOGLE_API_|IO_WORKBENCH_)/.test(key)) delete env[key];
}
// This is the child test process's home, never the interactive user's shell.
Object.assign(env, {
  HOME: root, TMPDIR: root,
  RUSTUP_HOME: process.env.RUSTUP_HOME || join(homedir(), '.rustup'),
  CARGO_HOME: process.env.CARGO_HOME || join(homedir(), '.cargo'),
  GRADLE_USER_HOME: process.env.GRADLE_USER_HOME || join(homedir(), '.gradle'),
  PLAYWRIGHT_BROWSERS_PATH: process.env.PLAYWRIGHT_BROWSERS_PATH || join(homedir(), process.platform === 'darwin' ? 'Library/Caches/ms-playwright' : '.cache/ms-playwright'),
  HTTP_PROXY: 'http://127.0.0.1:1', HTTPS_PROXY: 'http://127.0.0.1:1', NO_PROXY: '127.0.0.1,localhost',
  GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1',
  IOWB_TEST_BINARY: join(repository, 'target/debug/io-workbench'),
});
const missingClis = join(root, '.local/bin');
await mkdir(missingClis, { recursive: true });
for (const provider of ['codex', 'claude', 'gemini']) await writeFile(join(missingClis, provider), '#!/bin/sh\nexit 127\n', { mode: 0o700 });
env.PATH = `${missingClis}:${process.env.PATH}`;
const toolchain = process.env.IOWB_TEST_TOOLCHAIN || '1.93.1';

async function run(command, args, cwd = repository) {
  process.stdout.write(`\nRunning ${command} ${args.join(' ')}\n`);
  const code = await new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, env, stdio: 'inherit' });
    child.once('error', reject); child.once('exit', (code, signal) => resolve(signal ? 1 : code));
  });
  if (code !== 0) throw new Error(`${command} failed with exit code ${code}`);
}
try {
  await run('cargo', [`+${toolchain}`, 'test', '--offline', '-p', 'iowb-core', '-p', 'iowb-protocol', '-p', 'iowb-server', '-p', 'iowb-storage', '-p', 'iowb-tui', '-p', 'iowb-ui', '--lib', '--', '--test-threads=1', '--skip', 'fetch_all_remotes_matches_lazygit_and_preserves_fetch_head']);
  await run('cargo', [`+${toolchain}`, 'build', '--offline', '-p', 'iowb-cli']);
  const { readdir } = await import('node:fs/promises');
  for (const directory of ['crates/iowb-ui/tests', 'crates/iowb-server/tests']) {
    const tests = (await readdir(join(repository, directory))).filter(name => name.endsWith('.test.cjs')).sort().map(name => `${directory}/${name}`);
    await run(process.execPath, ['--test', '--test-concurrency=2', '--test-timeout=180000', ...tests]);
  }
  await run('gradle', ['--offline', ':shared:jvmTest', ':androidApp:compileDebugKotlin'], join(repository, 'apps/mobile'));
  process.stdout.write('\nAcceptance commands passed. Device, signer/update and unsupported-capability limitations remain as recorded in plan.md.\n');
} finally {
  await rm(root, { recursive: true, force: true });
}
