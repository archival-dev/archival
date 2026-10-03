// Hosts a site's carriers for `archival run`. The dev server spawns this and
// reverse-proxies /carriers/* to it.
//
// A carrier is built by the toolchain beside this file (toolchain.mjs, vendored
// from archival-dev/carriers-toolchain), which is the build every place that
// runs a carrier shares, and a request is parsed and answered through that
// toolchain's contract. That is what makes a carrier that works locally work
// once deployed.
//
// Imports nothing but node builtins and the toolchain: this file lives outside
// the site, so a bare specifier here would resolve against the wrong
// node_modules.

import http from "node:http";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";
import {
  buildCarrier,
  carrierFunction,
  parse,
  parseRequestBody,
  strip,
  toResponse,
} from "./toolchain.mjs";

export { parseRequestBody, toResponse };

const TOKEN = process.env.ARCHIVAL_CARRIER_TOKEN || "";
const PORT = Number(process.env.ARCHIVAL_CARRIER_PORT || 0);

const SHA_RE = /^[a-f0-9]{64}$/i;
const CARRIER_ROUTE = /^\/carrier\/([A-Za-z0-9_][A-Za-z0-9_.-]*)(?:\/.*)?$/;

const HERE = path.dirname(fileURLToPath(import.meta.url));
// Where this process writes the modules it builds. Node cannot import source
// it is only handed, and nothing generated may land in the site.
const BUILD_DIR = path.join(HERE, `build-${process.pid}`);
const BUILD_PREFIX = "build-";
// Package tarballs, kept across restarts: every carrier edit starts a new
// process, and a lockfile's tarballs never change.
const PACKAGE_CACHE = path.join(HERE, "packages");

// What a carrier's source can consist of. Everything else in its directory is
// left out of the build.
const SOURCE = /\.(?:ts|mts|cts|js|mjs|cjs|json)$/;
const DECLARATION = /\.d\.[mc]?ts$/;
// What a deploy writes its own output under, beside the carrier's source.
const DEPLOY_OUTPUT_PREFIX = "carrier_";

/** The most recent payload pushed by the dev server. */
let state = null;
/** carrier name -> build promise, so a carrier is only built and evaluated once. */
const modules = new Map();

const deepFreeze = (value) => {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const child of Object.values(value)) {
      deepFreeze(child);
    }
  }
  return value;
};

const uploadKey = (prefix, sha, filename) => {
  if (typeof sha !== "string" || !SHA_RE.test(sha)) {
    throw new Error("Invalid upload sha: " + sha);
  }
  if (typeof filename !== "string" || !filename) {
    throw new Error("An upload filename is required");
  }
  // The prefix is what scopes reads to this site, so nothing a carrier passes
  // may walk out of it.
  if (filename.startsWith("/") || filename.split("/").includes("..")) {
    throw new Error("Invalid upload filename: " + filename);
  }
  return (
    prefix + sha + "/" + filename.split("/").map(encodeURIComponent).join("/")
  );
};

/**
 * Builds objects.UPLOADS. Called per request so the filename index is memoized
 * for one request without carrying stale uploads into the next.
 */
const makeUploads = () => {
  const prefix = state.uploadPrefix;
  if (!prefix) {
    const unavailable = () => {
      throw new Error(
        "objects.UPLOADS is not available for this deploy (no upload prefix)",
      );
    };
    return { list: unavailable, get: unavailable };
  }
  // Locally there is no bucket to enumerate, so the dev server derives this
  // from the files the site's objects point at.
  const list = async () => state.uploads;
  const get = async (file, sha) => {
    let filename = file;
    let fileSha = sha;
    // A file value off objects already carries both halves.
    if (file && typeof file === "object") {
      filename = file.filename;
      fileSha = sha || file.sha;
    }
    if (typeof filename !== "string" || !filename) {
      throw new Error("UPLOADS.get needs a filename or a file object");
    }
    if (!fileSha) {
      const matches = (await list()).filter((e) => e.filename === filename);
      if (matches.length === 0) {
        return null;
      }
      if (matches.length > 1) {
        throw new Error(
          "More than one upload is named " +
            filename +
            " - pass a sha to say which one: " +
            matches.map((m) => m.sha).join(", "),
        );
      }
      fileSha = matches[0].sha;
    }
    const key = uploadKey(prefix, fileSha, filename);
    const url = state.uploadsUrl.replace(/\/$/, "") + "/" + key;
    const response = await fetch(url);
    if (response.status === 404) {
      return null;
    }
    if (!response.ok) {
      throw new Error(
        "Failed reading upload " + key + ": " + response.status + " from " + url,
      );
    }
    const bytes = new Uint8Array(await response.arrayBuffer());
    const etag = (response.headers.get("etag") || "").replace(/^"|"$/g, "");
    return {
      key,
      size: bytes.byteLength,
      etag,
      httpEtag: etag ? '"' + etag + '"' : "",
      get body() {
        return new Blob([bytes]).stream();
      },
      arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
      text: async () => new TextDecoder().decode(bytes),
      json: async () => JSON.parse(new TextDecoder().decode(bytes)),
      blob: async () => new Blob([bytes]),
      writeHttpMetadata: (headers) => {
        const type = response.headers.get("content-type");
        const encoding = response.headers.get("content-encoding");
        if (type) headers.set("content-type", type);
        if (encoding) headers.set("content-encoding", encoding);
      },
    };
  };
  return { list, get };
};

const readBody = (req) =>
  new Promise((resolve, reject) => {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => resolve(Buffer.concat(chunks)));
    req.on("error", reject);
  });

/**
 * A carrier's own files, by path relative to its directory. Installed
 * dependencies are not read from disk: the build installs what the lockfile
 * names, which is the same list wherever a carrier is built.
 */
export const readCarrierFiles = (root, base = "") => {
  const files = new Map();
  for (const entry of readdirSync(path.join(root, base), {
    withFileTypes: true,
  })) {
    if (
      entry.name.startsWith(DEPLOY_OUTPUT_PREFIX) ||
      entry.name.startsWith(".")
    ) {
      continue;
    }
    const relative = base ? `${base}/${entry.name}` : entry.name;
    if (entry.isDirectory()) {
      if (entry.name === "node_modules") {
        continue;
      }
      for (const [file, source] of readCarrierFiles(root, relative)) {
        files.set(file, source);
      }
    } else if (
      entry.isFile() &&
      SOURCE.test(entry.name) &&
      !DECLARATION.test(entry.name)
    ) {
      files.set(relative, readFileSync(path.join(root, relative), "utf-8"));
    }
  }
  return files;
};

/** fetch for a package tarball, answered from disk once it has been downloaded. */
const fetchPackage = async (url) => {
  const cached = path.join(
    PACKAGE_CACHE,
    createHash("sha256").update(url).digest("hex"),
  );
  if (existsSync(cached)) {
    return new Response(readFileSync(cached));
  }
  const response = await fetch(url);
  if (!response.ok) {
    return response;
  }
  const bytes = Buffer.from(await response.arrayBuffer());
  mkdirSync(PACKAGE_CACHE, { recursive: true });
  writeFileSync(cached, bytes);
  return new Response(bytes);
};

/**
 * Builds the carrier in `root` and writes its modules under `out`, answering
 * the function it default-exports.
 */
export const buildAndLoad = async (name, root, out) => {
  const compiled = await buildCarrier({
    files: readCarrierFiles(root),
    strip,
    parse,
    fetch: fetchPackage,
  });
  rmSync(out, { recursive: true, force: true });
  for (const [modulePath, code] of compiled.modules) {
    const file = path.join(out, ...modulePath.split("/"));
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, code, "utf-8");
  }
  // What makes node read the build's .js files as ES modules.
  writeFileSync(path.join(out, "package.json"), '{ "type": "module" }\n');
  const carrier = carrierFunction(
    await import(
      pathToFileURL(path.join(out, ...compiled.entry.split("/"))).href
    ),
  );
  if (!carrier) {
    throw new Error("it does not default-export a function");
  }
  return carrier;
};

const loadCarrier = (name) => {
  let loading = modules.get(name);
  if (!loading) {
    // Failures are memoized too: a carrier that fails to build or throws at
    // import time should report the same error on every request, not race a
    // half-loaded module.
    loading = buildAndLoad(
      name,
      state.carriers[name],
      path.join(BUILD_DIR, name),
    );
    modules.set(name, loading);
  }
  return loading;
};

const runCarrier = async (name, req, body) => {
  if (!state.carriers[name]) {
    return new Response("No carrier named " + name, { status: 404 });
  }
  const params = new URL(req.url, "http://carrier.local").searchParams;
  let carrier;
  try {
    carrier = await loadCarrier(name);
  } catch (e) {
    return new Response(
      "Carrier " + name + " could not be built: " + ((e && e.message) || e),
      { status: 500 },
    );
  }
  try {
    const parsed = await parseRequestBody(
      req.method,
      [["content-type", req.headers["content-type"] || ""]],
      body,
    );
    // UPLOADS has methods, so unlike the rest of the vars it cannot be part of
    // the pushed payload. It wins over a site object of the same name, which is
    // how every injected var resolves that collision.
    const objects = Object.freeze({
      ...state.objects,
      SITE_URL: state.siteUrl,
      UPLOADS: makeUploads(),
    });
    return toResponse(await carrier(params, parsed, objects));
  } catch (e) {
    return new Response("Carrier threw an error: " + e, { status: 500 });
  }
};

const send = async (res, response) => {
  res.statusCode = response.status;
  for (const [key, value] of response.headers.entries()) {
    if (key.toLowerCase() === "set-cookie") continue;
    res.setHeader(key, value);
  }
  const cookies = response.headers.getSetCookie?.() ?? [];
  if (cookies.length) {
    res.setHeader("set-cookie", cookies);
  }
  const bytes = Buffer.from(await response.arrayBuffer());
  res.end(bytes);
};

const authorized = (req) => TOKEN && req.headers["x-archival-token"] === TOKEN;

const server = http.createServer(async (req, res) => {
  try {
    const path = new URL(req.url, "http://carrier.local").pathname;
    if (path.startsWith("/__control/")) {
      if (!authorized(req)) {
        return send(res, new Response("Forbidden", { status: 403 }));
      }
      if (path === "/__control/health") {
        return send(
          res,
          Response.json({
            ok: true,
            carriers: state ? Object.keys(state.carriers) : [],
            hasState: !!state,
          }),
        );
      }
      if (path === "/__control/state" && req.method === "POST") {
        const pushed = JSON.parse((await readBody(req)).toString("utf-8"));
        // Carriers are re-imported on restart, not on a state push, so the
        // module cache is deliberately left alone here.
        state = { ...pushed, objects: deepFreeze(pushed.objects) };
        return send(res, Response.json({ ok: true }));
      }
      return send(res, new Response("Not Found", { status: 404 }));
    }
    const route = CARRIER_ROUTE.exec(path);
    if (!route) {
      return send(res, new Response("Not Found", { status: 404 }));
    }
    if (!state) {
      return send(res, new Response("Carriers are still starting", { status: 503 }));
    }
    const body = await readBody(req);
    return send(res, await runCarrier(route[1], req, body));
  } catch (e) {
    console.error("[carrier] request failed:", e);
    if (!res.headersSent) {
      res.statusCode = 500;
    }
    res.end("Carrier harness failed: " + e);
  }
});

/**
 * Node warns that its type stripper is experimental the first time a
 * TypeScript carrier is built. Every other warning is the carrier author's to
 * read, so only that one is dropped.
 */
const quietTypeStripping = () => {
  const [print] = process.listeners("warning");
  process.removeAllListeners("warning");
  process.on("warning", (warning) => {
    if (
      warning.name === "ExperimentalWarning" &&
      /stripTypeScriptTypes/.test(warning.message)
    ) {
      return;
    }
    print?.(warning);
  });
};

/** Removes what earlier sidecars for this site built, and makes room for this one's. */
const prepareBuildDir = () => {
  for (const entry of readdirSync(HERE, { withFileTypes: true })) {
    if (entry.isDirectory() && entry.name.startsWith(BUILD_PREFIX)) {
      rmSync(path.join(HERE, entry.name), { recursive: true, force: true });
    }
  }
  mkdirSync(BUILD_DIR, { recursive: true });
};

export const start = () => {
  // One carrier's unhandled failure must not take down the others.
  process.on("uncaughtException", (e) => console.error("[carrier]", e));
  process.on("unhandledRejection", (e) => console.error("[carrier]", e));
  quietTypeStripping();
  prepareBuildDir();

  // The dev server holds this pipe open and never writes to it, so this fires
  // even when the parent dies in a way that runs no cleanup.
  process.stdin.on("end", () => process.exit(0));
  process.stdin.on("close", () => process.exit(0));
  process.stdin.resume();

  server.listen(PORT, "127.0.0.1", () => {
    // The dev server reads the port from here rather than picking one itself,
    // so there is no window in which the port is taken by something else.
    console.log(
      JSON.stringify({ archivalCarrier: { port: server.address().port } }),
    );
  });
};

// Both sides are resolved before comparing: node resolves symlinks when it
// loads a module, but argv[1] is whatever the caller passed, and the harness
// lives under a temp directory that is a symlink on macOS.
const isEntrypoint = () => {
  if (!process.argv[1]) return false;
  try {
    return (
      realpathSync(fileURLToPath(import.meta.url)) ===
      realpathSync(process.argv[1])
    );
  } catch {
    return false;
  }
};

// Importing this file (the tests do) must not bind a port or hold the process
// open, so nothing starts until it is run as the entrypoint.
if (isEntrypoint()) {
  start();
}
