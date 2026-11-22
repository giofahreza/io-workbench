import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFileSync, spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const installer = fileURLToPath(new URL("../install.sh", import.meta.url));
const termuxInstaller = fileURLToPath(new URL("../install-termux.sh", import.meta.url));

function fixture() {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "iowb-install-")));
  const home = join(root, "home");
  const bin = join(root, "mock-bin");
  const release = join(root, "release");
  for (const dir of [home, bin, release]) mkdirSync(dir);
  for (const name of ["io-workbench", "iowb"]) {
    writeFileSync(join(release, name), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
  }
  const archive = join(root, "release.tar.gz");
  execFileSync("tar", ["-czf", archive, "-C", release, "io-workbench", "iowb"]);
  const digest = createHash("sha256").update(readFileSync(archive)).digest("hex");
  const curl = join(bin, "curl");
  writeFileSync(curl, `#!/usr/bin/env node
const fs = require('node:fs');
const args = process.argv.slice(2);
const output = args[args.indexOf('--output') + 1];
const asset = args.at(-1).split('/').at(-1);
if (asset === 'SHA256SUMS') {
  const name = fs.readFileSync(${JSON.stringify(join(root, "asset-name"))}, 'utf8');
  fs.writeFileSync(output, ${JSON.stringify(digest)} + '  ' + name + '\\n');
} else {
  fs.writeFileSync(${JSON.stringify(join(root, "asset-name"))}, asset);
  fs.copyFileSync(${JSON.stringify(archive)}, output);
}
`);
  chmodSync(curl, 0o755);
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("IO_WORKBENCH_")));
  return {
    home,
    run: (args = [], overrides = {}) => spawnSync("sh", [installer, "--non-interactive", "--version", "v0.0.0", ...args], {
      env: { ...env, HOME: home, PATH: `${bin}:${process.env.PATH}`, ...overrides }, encoding: "utf8",
    }),
    cleanup: () => rmSync(root, { recursive: true, force: true }),
  };
}

test("fresh install creates ~/dev without requiring an existing workspace", () => {
  const f = fixture();
  try {
    assert.equal(existsSync(join(f.home, "dev")), false);
    const result = f.run();
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.ok(existsSync(join(f.home, "dev")));
    assert.ok(existsSync(join(f.home, ".local/bin/io-workbench")));
    assert.ok(result.stdout.includes(`Workspace authority: ${f.home}/dev`));
  } finally { f.cleanup(); }
});

test("explicit workspace overrides remain authoritative", () => {
  for (const useEnv of [false, true]) {
    const f = fixture();
    try {
      const custom = join(f.home, "custom workspace");
      mkdirSync(custom);
      const result = useEnv ? f.run([], { IO_WORKBENCH_WORKSPACE_ROOT: custom }) : f.run(["--workspace-root", custom]);
      assert.equal(result.status, 0, result.stdout + result.stderr);
      assert.ok(result.stdout.includes(`Workspace authority: ${custom}`));
      assert.equal(existsSync(join(f.home, "dev")), false);
    } finally { f.cleanup(); }
  }
});

test("an explicitly missing workspace still fails without creating it", () => {
  const f = fixture();
  try {
    const missing = join(f.home, "missing");
    const result = f.run(["--workspace-root", missing]);
    assert.notEqual(result.status, 0);
    assert.ok(result.stderr.includes("workspace root does not exist"));
    assert.equal(existsSync(missing), false);
  } finally { f.cleanup(); }
});

test("Termux bootstrap uses ~/dev for a fresh install and preserves saved or explicit paths", () => {
  for (const mode of ["fresh", "saved", "environment", "flag"]) {
    const root = realpathSync(mkdtempSync(join(tmpdir(), "iowb-termux-workspace-")));
    try {
      const prefix = join(root, "com.termux/files/usr");
      mkdirSync(join(prefix, "bin"), { recursive: true });
      // Stop before any package or network activity, after directory selection.
      writeFileSync(join(prefix, "bin/uname"), "#!/bin/sh\nprintf unsupported-test-abi\n", { mode: 0o755 });
      mkdirSync(join(root, ".io-workbench"));
      if (mode !== "fresh") writeFileSync(join(root, ".io-workbench/termux-runtime-workspace-root"), `${root}/saved\n`);
      const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("IO_WORKBENCH_")));
      Object.assign(env, { HOME: root, PREFIX: prefix });
      if (mode === "environment") env.IO_WORKBENCH_WORKSPACE_ROOT = join(root, "environment");
      const args = mode === "flag" ? ["--workspace-root", join(root, "flag")] : [];
      const result = spawnSync("sh", [termuxInstaller, "--no-start", ...args], { env, encoding: "utf8" });
      assert.notEqual(result.status, 0);
      assert.ok(result.stderr.includes("no verified Termux runtime is published"), result.stdout + result.stderr);
      const chosen = mode === "fresh" ? "dev" : mode;
      assert.ok(existsSync(join(root, chosen)));
      if (mode !== "fresh") assert.equal(existsSync(join(root, "dev")), false);
    } finally { rmSync(root, { recursive: true, force: true }); }
  }
});
