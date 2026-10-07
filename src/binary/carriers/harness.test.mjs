// Covers what the harness adds around the toolchain: which files of a carrier
// directory are its source, and that a carrier is built and loaded the way the
// toolchain says rather than by node's own loader.
//
// Run with: node --test src/binary/carriers/harness.test.mjs

import assert from "node:assert/strict";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import {
  buildAndLoad,
  carrierApiVersion,
  closeDatabases,
  invokeCarrier,
  parseRequestBody,
  readCarrierFiles,
  toResponse,
} from "./harness.mjs";
import * as toolchain from "./toolchain.mjs";

const dirs = [];
test.after(async () => {
  await closeDatabases();
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

const sqlite = {
  skip:
    !process.getBuiltinModule?.("node:sqlite") && "this node has no node:sqlite",
};

/** A version 2 carrier's `site`, over a database file that does not exist yet. */
const siteWithDatabase = async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "archival-harness-"));
  dirs.push(dir);
  const database = path.join(dir, "dist", "site.sqlite");
  const [, , , site] = await invokeCarrier(
    { carrier: (...args) => args, api: 2 },
    new URLSearchParams(),
    null,
    { objects: {}, siteUrl: "", uploads: [], database },
  );
  return { site, database };
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
  const { carrier: run, api } = await buildAndLoad("echo", root, out);
  assert.equal(run(new URLSearchParams("name=sam")), "hi sam!");
  assert.equal(api, 1, "a carrier that names no version is version 1");
});

test("a carrier names the carrier API version it is written against", () => {
  assert.equal(carrierApiVersion(undefined), 1);
  assert.equal(carrierApiVersion("{}"), 1);
  assert.equal(carrierApiVersion('{"archival":{"carrier":1}}'), 1);
  assert.equal(carrierApiVersion('{"archival":{"carrier":2}}'), 2);
  assert.throws(
    () => carrierApiVersion('{"archival":{"carrier":99}}'),
    /asks for carrier API 99/,
  );
  assert.throws(() => carrierApiVersion("{"), /not valid JSON/);
});

test("each carrier API version is called its own way", async () => {
  const current = {
    objects: Object.freeze({ artist: [{ name: "Tormenta Rey" }] }),
    siteUrl: "http://localhost:1234",
    uploads: [],
    uploadPrefix: "",
    uploadsUrl: "",
  };
  const echo = (...args) => args;
  const [, , v1Objects, v1Site] = await invokeCarrier(
    { carrier: echo, api: 1 },
    new URLSearchParams(),
    null,
    current,
  );
  assert.equal(v1Objects.SITE_URL, "http://localhost:1234");
  assert.equal(v1Objects.artist[0].name, "Tormenta Rey");
  assert.equal(v1Site, undefined);

  const [, , v2Objects, site] = await invokeCarrier(
    { carrier: echo, api: 2 },
    new URLSearchParams(),
    null,
    current,
  );
  assert.equal(v2Objects, current.objects);
  assert.equal(site.url, "http://localhost:1234");
  assert.ok(Object.isFrozen(site));
  assert.throws(() => site.uploads.list(), /site\.uploads is not available/);
  await assert.rejects(
    site.email.send({ to: "a@b.test", subject: "x", text: "y" }),
    /site\.email is not available in archival run/,
  );
  await assert.rejects(
    site.sql.exec("SELECT 1"),
    /site\.sql is not available in archival run/,
    "without a database file to open",
  );
  assert.equal(await site.activitypub.account(), null);
  assert.deepEqual(await site.activitypub.replies({ path: "artist/a" }), {
    items: [],
    total: 0,
    next: null,
  });
  await assert.rejects(
    site.activitypub.likes({ name: "no path" }),
    /site\.activitypub needs a post/,
  );
});

test("site.sql keeps what it is given in its database file", sqlite, async () => {
  const { site, database } = await siteWithDatabase();
  assert.deepEqual(
    await site.sql.exec(
      "CREATE TABLE signups (email TEXT, confirmed INTEGER, at TEXT, avatar BLOB);" +
        " INSERT INTO signups VALUES (?, ?, ?, ?); -- the last statement binds",
      "a@b.test",
      true,
      new Date(Date.UTC(2026, 9, 6)),
      new Uint8Array([1, 2, 3]),
    ),
    [],
  );
  assert.ok(existsSync(database));
  const [row] = await site.sql.exec("SELECT * FROM signups");
  assert.equal(Object.getPrototypeOf(row), Object.prototype);
  assert.deepEqual(row, {
    email: "a@b.test",
    confirmed: 1,
    at: "2026-10-06T00:00:00.000Z",
    avatar: new Uint8Array([1, 2, 3]),
  });

  await assert.rejects(
    site.sql.transaction([
      ["INSERT INTO signups (email) VALUES (?)", "c@d.test"],
      ["INSERT INTO nowhere VALUES (1)"],
    ]),
    /site\.sql failed: no such table: nowhere/,
  );
  assert.deepEqual(
    await site.sql.transaction([
      ["SELECT count(*) AS count FROM signups"],
      ["SELECT email FROM signups WHERE confirmed = ?", true],
    ]),
    [[{ count: 1 }], [{ email: "a@b.test" }]],
    "a failed transaction keeps nothing it did",
  );

  const { DatabaseSync } = process.getBuiltinModule("node:sqlite");
  const reopened = new DatabaseSync(database, { readOnly: true });
  assert.equal(
    reopened.prepare("SELECT email FROM signups").get().email,
    "a@b.test",
    "a committed write is on disk",
  );
  reopened.close();
});

test("site.sql refuses what a deployed database refuses", sqlite, async () => {
  const { site } = await siteWithDatabase();
  await assert.rejects(
    site.sql.exec("SELECT ?", undefined),
    /bind null instead/,
  );
  await assert.rejects(
    site.sql.exec("SELECT ?", { an: "object" }),
    /site\.sql failed: Parameter 1 of statement 1 must be/,
  );
  await assert.rejects(site.sql.exec("  "), /Statement 1 has no SQL/);
  await assert.rejects(
    site.sql.exec("SELECT 1", ...new Array(101).fill(1)),
    /at most 100 parameters/,
  );
  await assert.rejects(site.sql.transaction("SELECT 1"), /takes a list/);
  await assert.rejects(site.sql.transaction([]), /non-empty/);
  await assert.rejects(
    site.sql.exec(
      "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 10000)" +
        " SELECT i, zeroblob(1000) AS padding FROM n",
    ),
    /results can be at most/,
  );
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
  const future = carrier({
    "index.js": `export default () => 1;`,
    "package.json": `{"archival":{"carrier":99}}`,
  });
  await assert.rejects(
    buildAndLoad("future", future.root, future.out),
    /asks for carrier API 99/,
  );
  const value = carrier({ "index.js": `export default 42;` });
  await assert.rejects(
    buildAndLoad("value", value.root, value.out),
    /default-export a function/,
  );
});
