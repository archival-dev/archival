// Covers what the harness adds around the toolchain: which files of a carrier
// directory are its source, and that a carrier is built and loaded the way the
// toolchain says rather than by node's own loader.
//
// Run with: node --test src/binary/carriers/harness.test.mjs

import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import {
  buildAndLoad,
  parseRequestBody,
  readCarrierFiles,
  toResponse,
} from "./harness.mjs";
import * as toolchain from "./toolchain.mjs";

const dirs = [];
test.after(() => {
  for (const dir of dirs) {
    rmSync(dir, { recursive: true, force: true });
  }
});

const carrier = (files) => {
  const dir = mkdtempSync(path.join(tmpdir(), "archival-harness-"));
  dirs.push(dir);
  for (const [file, source] of Object.entries(files)) {
    const target = path.join(dir, "src", file);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, source);
  }
  return { root: path.join(dir, "src"), out: path.join(dir, "out") };
};

const typescript = {
  skip: !toolchain.supported && "this node cannot strip types",
};

test("a request is parsed and answered by the toolchain's contract", async () => {
  assert.equal(parseRequestBody, toolchain.parseRequestBody);
  assert.equal(toResponse, toolchain.toResponse);
  assert.deepEqual(
    await parseRequestBody(
      "POST",
      [["content-type", "application/json; charset=utf-8"]],
      Buffer.from('{"a":1}'),
    ),
    { a: 1 },
  );
  assert.equal(toResponse("redirect:/login").headers.get("location"), "/login");
});

test("a carrier's source leaves out what a build never reads", () => {
  const { root } = carrier({
    "index.ts": "",
    "lib/util.js": "",
    "data.json": "{}",
    "package.json": "{}",
    "archival-objects.d.ts": "",
    "README.md": "",
    "node_modules/dep/index.js": "",
    ".cache/x.js": "",
    "carrier_abc/index.ts.js": "",
    "carrier_abc.jsonc": "",
  });
  assert.deepEqual([...readCarrierFiles(root).keys()].sort(), [
    "data.json",
    "index.ts",
    "lib/util.js",
    "package.json",
  ]);
});

test("a typescript carrier is built and loaded", typescript, async () => {
  const { root, out } = carrier({
    "index.ts": `import { greet } from "./lib/greet";\nimport data from "./data.json";\nexport default (params: URLSearchParams) => greet(params.get("name")) + data.mark;`,
    "lib/greet.ts": `export const greet = (name: string | null): string => "hi " + name;`,
    "data.json": `{"mark":"!"}`,
  });
  const run = await buildAndLoad("echo", root, out);
  assert.equal(run(new URLSearchParams("name=sam")), "hi sam!");
});

test("what a deploy would refuse is refused here, by name", async () => {
  const commonjs = carrier({ "index.js": `module.exports = () => 1;` });
  await assert.rejects(
    buildAndLoad("commonjs", commonjs.root, commonjs.out),
    /CommonJS/,
  );
  const builtin = carrier({
    "index.js": `import fs from "node:fs";\nexport default () => fs;`,
  });
  await assert.rejects(
    buildAndLoad("builtin", builtin.root, builtin.out),
    /node:fs.*Node built-in/s,
  );
  const unlocked = carrier({
    "index.js": `import pad from "left-pad";\nexport default () => pad;`,
    "package.json": `{"dependencies":{"left-pad":"1.3.0"}}`,
  });
  await assert.rejects(
    buildAndLoad("unlocked", unlocked.root, unlocked.out),
    /package-lock\.json/,
  );
  const value = carrier({ "index.js": `export default 42;` });
  await assert.rejects(
    buildAndLoad("value", value.root, value.out),
    /default-export a function/,
  );
});
