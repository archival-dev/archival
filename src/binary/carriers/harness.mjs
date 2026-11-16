// Hosts a site's carriers for `archival run`. The dev server spawns this and
// reverse-proxies /carriers/* to it, along with the cart's script and checkout
// under /_archival/ when the run has a payment account.
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
import { spawn } from "node:child_process";
import { createHash, randomBytes, randomUUID } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  statSync,
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
 * Builds the uploads facade, which the carrier calls `name`, over `current`,
 * the pushed state. Called per request so the filename index is memoized for
 * one request without carrying stale uploads into the next.
 */
const makeUploads = (name, current) => {
  const prefix = current.uploadPrefix;
  if (!prefix) {
    const unavailable = () => {
      throw new Error(
        name + " is not available for this deploy (no upload prefix)",
      );
    };
    return { list: unavailable, get: unavailable };
  }
  // Locally there is no bucket to enumerate, so the dev server derives this
  // from the files the site's objects point at.
  const list = async () => current.uploads;
  const get = async (file, sha) => {
    let filename = file;
    let fileSha = sha;
    // A file value off objects already carries both halves.
    if (file && typeof file === "object") {
      filename = file.filename;
      fileSha = sha || file.sha;
    }
    if (typeof filename !== "string" || !filename) {
      throw new Error(name + ".get needs a filename or a file object");
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
    const url = current.uploadsUrl.replace(/\/$/, "") + "/" + key;
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

// What a deployed site's mail refuses, before it sends anything.
const EMAIL_MAX_RECIPIENTS = 10;
const EMAIL_MAX_SUBJECT_LENGTH = 500;
const EMAIL_MAX_BODY_BYTES = 256 * 1024;
const EMAIL_ADDRESS =
  /^[a-z0-9!#$%&'*+/=?^_`{|}~-]+(?:\.[a-z0-9!#$%&'*+/=?^_`{|}~-]+)*@[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)+$/i;

const isEmailAddress = (value) =>
  typeof value === "string" && value.length <= 254 && EMAIL_ADDRESS.test(value);

/** The message a deployed send would accept, or the reason it would not. */
const readEmail = (message) => {
  if (!message || typeof message !== "object" || Array.isArray(message)) {
    return { error: "the message must be an object" };
  }
  const to = Array.isArray(message.to) ? message.to : [message.to];
  if (to.length === 0) {
    return { error: "to is required" };
  }
  if (to.length > EMAIL_MAX_RECIPIENTS) {
    return {
      error: `A message can have at most ${EMAIL_MAX_RECIPIENTS} recipients`,
    };
  }
  const invalid = to.find((recipient) => !isEmailAddress(recipient));
  if (invalid !== undefined) {
    return { error: `"${String(invalid)}" is not an email address` };
  }
  const { subject, text, html, from, replyTo } = message;
  if (typeof subject !== "string" || !subject.trim()) {
    return { error: "subject is required" };
  }
  if (subject.length > EMAIL_MAX_SUBJECT_LENGTH) {
    return {
      error: `subject can be at most ${EMAIL_MAX_SUBJECT_LENGTH} characters`,
    };
  }
  if (/[\r\n]/.test(subject)) {
    return { error: "subject cannot contain line breaks" };
  }
  for (const [field, value] of [
    ["text", text],
    ["html", html],
  ]) {
    if (value !== undefined && value !== null && typeof value !== "string") {
      return { error: `${field} must be a string` };
    }
  }
  if ((text ?? undefined) === undefined && (html ?? undefined) === undefined) {
    return { error: "text or html is required" };
  }
  if ((text?.length ?? 0) + (html?.length ?? 0) > EMAIL_MAX_BODY_BYTES) {
    return { error: `The body can be at most ${EMAIL_MAX_BODY_BYTES} bytes` };
  }
  for (const value of [from, replyTo]) {
    if (value !== undefined && value !== null && !isEmailAddress(value)) {
      return { error: `"${String(value)}" is not an email address` };
    }
  }
  return {
    email: {
      from: from ? from.toLowerCase() : null,
      to,
      subject,
      text: text ?? null,
      html: html ?? null,
      replyTo: replyTo ?? null,
    },
  };
};

/** id -> an email a carrier sent, kept until this process exits. */
const emails = new Map();

const EMAIL_PATH = /^\/__email\/([0-9a-f-]{36})$/;

/** Where the email `id` can be read, once the harness is listening. */
const emailUrl = (id) => {
  const address = server.listening ? server.address() : null;
  return address ? `http://127.0.0.1:${address.port}/__email/${id}` : null;
};

const openInBrowser = (url) => {
  if (process.env.ARCHIVAL_OPEN_EMAILS === "0") {
    return;
  }
  const [command, args] =
    process.platform === "darwin"
      ? ["open", [url]]
      : process.platform === "win32"
        ? ["cmd", ["/c", "start", "", url]]
        : ["xdg-open", [url]];
  try {
    const child = spawn(command, args, { stdio: "ignore", detached: true });
    child.on("error", () => {});
    child.unref();
  } catch {
    // The url is logged as well, for a machine with no browser to open.
  }
};

/**
 * Builds the email facade, which the carrier calls `name`. A message is
 * refused as a deployed send refuses it, and opened in a browser window
 * instead of being sent. Only a deployed site knows its addresses, so with no
 * `from` named the receipt's `from` is empty.
 */
const makeEmail = (name, carrier) => ({
  send: async (message) => {
    const read = readEmail(message);
    if (read.error) {
      throw new Error("Sending email failed: " + read.error);
    }
    const id = randomUUID();
    emails.set(id, { ...read.email, id, carrier, sentAt: Date.now() });
    const url = emailUrl(id);
    if (url) {
      console.log(
        `[carrier] ${carrier} sent "${read.email.subject}" (${name}): ${url}`,
      );
      openInBrowser(url);
    }
    return { id, from: read.email.from ?? "" };
  },
});

const escapeHtml = (value) =>
  String(value).replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );

/**
 * The page an email opens on. The body is the carrier's, so it renders in a
 * frame sandboxed without scripts, as a mail client would show it.
 */
export const emailPage = (email) => {
  const row = (label, value) =>
    `<div class="row"><dt>${label}</dt><dd>${escapeHtml(value)}</dd></div>`;
  const parts = [];
  if (email.html !== null) {
    parts.push(
      `<iframe class="body" sandbox="allow-popups allow-popups-to-escape-sandbox" referrerpolicy="no-referrer" title="Email body" srcdoc="${escapeHtml(`<base target="_blank">${email.html}`)}"></iframe>`,
    );
  }
  if (email.text !== null) {
    parts.push(
      email.html === null
        ? `<pre class="body">${escapeHtml(email.text)}</pre>`
        : `<details><summary>Plain text</summary><pre class="body">${escapeHtml(email.text)}</pre></details>`,
    );
  }
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escapeHtml(email.subject)}</title>
<style>
:root { color-scheme: light dark; }
body { margin: 0; padding: 16px; font: 14px/1.5 system-ui, sans-serif; display: flex; flex-direction: column; gap: 12px; min-height: calc(100vh - 32px); box-sizing: border-box; }
.notice { margin: 0; opacity: 0.7; }
dl { margin: 0; display: grid; gap: 4px; }
.row { display: flex; gap: 8px; }
dt { width: 72px; flex-shrink: 0; opacity: 0.7; }
dd { margin: 0; overflow-wrap: anywhere; }
.body { border: 1px solid #8884; border-radius: 8px; }
iframe.body { flex: 1; min-height: 480px; width: 100%; background: #fff; }
pre.body { margin: 0; padding: 12px; white-space: pre-wrap; font: inherit; }
</style>
</head>
<body>
<p class="notice">Preview: the ${escapeHtml(email.carrier)} carrier sent this at ${escapeHtml(new Date(email.sentAt).toLocaleTimeString())}. Nothing was delivered.</p>
<dl>
${row("From", email.from ?? "Your site's first email address")}
${row("To", email.to.join(", "))}
${email.replyTo ? row("Reply-To", email.replyTo) : ""}
${row("Subject", email.subject)}
</dl>
${parts.join("\n")}
</body>
</html>
`;
};

const emailResponse = (id) => {
  const email = emails.get(id);
  if (!email) {
    return new Response(
      "This email is gone: it was kept only while archival run was running.",
      { status: 404, headers: { "content-type": "text/plain; charset=utf-8" } },
    );
  }
  return new Response(emailPage(email), {
    headers: {
      "content-type": "text/html; charset=utf-8",
      "content-security-policy":
        "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src * data:; font-src * data:; frame-src 'self'",
      "cache-control": "no-store",
    },
  });
};

// What a deployed site's database refuses a request for, before it runs one.
const SQL_MAX_STATEMENTS = 100;
const SQL_MAX_STATEMENT_LENGTH = 100_000;
const SQL_MAX_PARAMS = 100;
const SQL_MAX_RESULT_BYTES = 5 * 1024 * 1024;

// What may sit between two statements in one string without being one.
const BETWEEN_STATEMENTS = /^(?:\s|;|--[^\n]*|\/\*[\s\S]*?(?:\*\/|$))*/;

/** database file -> its connection, which every carrier and request shares. */
const databases = new Map();

const openDatabase = (name, file) => {
  let opening = databases.get(file);
  if (!opening) {
    opening = import("node:sqlite").then(
      ({ DatabaseSync }) => {
        mkdirSync(path.dirname(file), { recursive: true });
        return new DatabaseSync(file);
      },
      () => {
        throw new Error(
          name +
            " needs Node 22.13 or later, which has node:sqlite; this is Node " +
            process.version,
        );
      },
    );
    databases.set(file, opening);
  }
  return opening;
};

/** Windows will not remove a database file while a connection holds it open. */
export const closeDatabases = async () => {
  const opening = [...databases.values()];
  databases.clear();
  for (const result of await Promise.allSettled(opening)) {
    if (result.status === "fulfilled") {
      result.value.close();
    }
  }
};

/**
 * A bound value as a deployed carrier's database receives it, which is after
 * a round trip through JSON: a Date arrives as its ISO string, NaN as null.
 */
const toParam = (name, value) => {
  if (value instanceof ArrayBuffer) {
    return new Uint8Array(value);
  }
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }
  if (value === undefined) {
    throw new Error(name + " cannot bind undefined - bind null instead");
  }
  const sent = JSON.parse(JSON.stringify([value]))[0];
  if (typeof sent === "boolean") {
    return sent ? 1 : 0;
  }
  if (sent === null || typeof sent === "string" || typeof sent === "number") {
    return sent;
  }
  return undefined;
};

const valueBytes = (value) =>
  value instanceof Uint8Array
    ? value.byteLength
    : typeof value === "string"
      ? value.length
      : 8;

/**
 * Runs every statement in `sql`, binding `params` to the last and answering
 * its rows, as a deployed site's database does. node:sqlite prepares only the
 * first statement in a string, so the rest are found by what it consumed.
 */
const runStatements = (db, sql, params) => {
  let rows = [];
  let rest = sql.replace(BETWEEN_STATEMENTS, "");
  while (rest) {
    const statement = db.prepare(rest);
    rest = rest
      .slice(statement.sourceSQL.length)
      .replace(BETWEEN_STATEMENTS, "");
    if (rest) {
      statement.run();
    } else {
      rows = statement.all(...params);
    }
  }
  return rows;
};

/**
 * Builds the sql facade, which the carrier calls `name`, over the SQLite file
 * at `file`. Each call is one transaction, as it is deployed; the plan and
 * hourly allowances a deployed site's database enforces are not.
 */
const makeSql = (name, file) => {
  const run = async (statements) => {
    if (!file) {
      throw new Error(name + " is not available in archival run");
    }
    const requested = statements.map(([sql, ...params]) => ({
      sql,
      params: params.map((param) => toParam(name, param)),
    }));
    const refuse = (error) => {
      throw new Error(name + " failed: " + error);
    };
    if (requested.length === 0) {
      refuse("statements must be a non-empty array");
    }
    if (requested.length > SQL_MAX_STATEMENTS) {
      refuse(`A transaction takes at most ${SQL_MAX_STATEMENTS} statements`);
    }
    for (const [i, { sql, params }] of requested.entries()) {
      if (typeof sql !== "string" || !sql.trim()) {
        refuse(`Statement ${i + 1} has no SQL`);
      }
      if (sql.length > SQL_MAX_STATEMENT_LENGTH) {
        refuse(
          `Statement ${i + 1} is longer than ${SQL_MAX_STATEMENT_LENGTH} characters`,
        );
      }
      if (params.length > SQL_MAX_PARAMS) {
        refuse(`Statement ${i + 1} takes at most ${SQL_MAX_PARAMS} parameters`);
      }
      const p = params.indexOf(undefined);
      if (p !== -1) {
        refuse(
          `Parameter ${p + 1} of statement ${i + 1} must be a string, a finite number, a boolean, null or bytes`,
        );
      }
    }
    const db = await openDatabase(name, file);
    let resultBytes = 0;
    db.exec("BEGIN");
    try {
      const results = requested.map(({ sql, params }) =>
        runStatements(db, sql, params).map((row) => {
          for (const [column, value] of Object.entries(row)) {
            resultBytes += column.length + valueBytes(value);
          }
          if (resultBytes > SQL_MAX_RESULT_BYTES) {
            throw new Error(
              `A request's results can be at most ${SQL_MAX_RESULT_BYTES} bytes. Select fewer rows, or fewer columns.`,
            );
          }
          return { ...row };
        }),
      );
      db.exec("COMMIT");
      return results;
    } catch (e) {
      try {
        db.exec("ROLLBACK");
      } catch {
        // The statement that failed already ended the transaction.
      }
      refuse((e && e.message) || e);
    }
  };
  const exec = async (sql, ...params) => (await run([[sql, ...params]]))[0];
  const transaction = async (statements) => {
    if (
      !Array.isArray(statements) ||
      statements.some((statement) => !Array.isArray(statement))
    ) {
      throw new Error(
        name + ".transaction takes a list of [sql, ...params] statements",
      );
    }
    return run(statements);
  };
  return { exec, transaction };
};

/**
 * A local run has no fediverse presence, so it reads as a site that does not
 * federate, as a deployed carrier's facade does with no site behind it.
 */
const makeActivityPub = (name) => {
  const postPath = (post) => {
    const path = post && typeof post === "object" ? post.path : post;
    if (typeof path !== "string" || !path) {
      throw new Error(name + " needs a post: one of your objects, or its path");
    }
    return path;
  };
  const emptyPage = async (post) => {
    postPath(post);
    return { items: [], total: 0, next: null };
  };
  return Object.freeze({
    account: async () => null,
    accounts: async () => [],
    followers: async () => ({ items: [], total: 0, next: null }),
    counts: async (posts) => {
      if (!Array.isArray(posts)) {
        throw new Error(name + ".counts takes a list of posts");
      }
      return posts.map((post) => {
        postPath(post);
        return { likes: 0, boosts: 0, replies: 0 };
      });
    },
    likes: emptyPage,
    boosts: emptyPage,
    replies: emptyPage,
    setLocalLikes: async (post, likes) => {
      postPath(post);
      if (!Number.isSafeInteger(likes) || likes < 0) {
        throw new Error(
          name + ".setLocalLikes needs a whole number of likes, 0 or more",
        );
      }
    },
  });
};

/**
 * How a carrier written against each carrier API version is called, given the
 * pushed state. A carrier names its version in its package.json, as
 * `"archival": { "carrier": 2 }`.
 */
/**
 * A local run has received no mentions, so it reads as a site none has
 * reached, as a deployed carrier's facade does with no site behind it.
 */
const makeWebmentions = (name) => {
  const pagePath = (page) => {
    const path = page && typeof page === "object" ? page.path : page;
    if (typeof path !== "string") {
      throw new Error(name + " needs a page: one of your objects, or a path");
    }
    return path;
  };
  const noCounts = () => ({
    replies: 0,
    likes: 0,
    reposts: 0,
    bookmarks: 0,
    mentions: 0,
  });
  return Object.freeze({
    counts: async (pages) => {
      if (!Array.isArray(pages)) {
        throw new Error(name + ".counts takes a list of pages");
      }
      return pages.map((page) => {
        pagePath(page);
        return noCounts();
      });
    },
    list: async (page) => {
      pagePath(page);
      return { items: [], total: 0, next: null };
    },
  });
};

/**
 * A local run has no ATProto account, so it reads as a site without one, as
 * a deployed carrier's facade does with no site behind it.
 */
const makeATProto = (name) => {
  const postPath = (post) => {
    const path = post && typeof post === "object" ? post.path : post;
    if (typeof path !== "string" || !path) {
      throw new Error(name + " needs a post: one of your objects, or its path");
    }
    return path;
  };
  const emptyPage = async (post) => {
    postPath(post);
    return { items: [], total: 0, next: null };
  };
  return Object.freeze({
    account: async () => null,
    followers: async () => ({ items: [], total: 0, next: null }),
    counts: async (posts) => {
      if (!Array.isArray(posts)) {
        throw new Error(name + ".counts takes a list of posts");
      }
      return posts.map((post) => {
        postPath(post);
        return { likes: 0, reposts: 0, replies: 0 };
      });
    },
    likes: emptyPage,
    reposts: emptyPage,
    replies: emptyPage,
  });
};

/**
 * A local run has no interactions on any network, so every post reads as one
 * nothing has reached, as a deployed carrier's facade does with no site behind it.
 */
const makeInteractions = (name) => {
  const postPath = (post) => {
    const path = post && typeof post === "object" ? post.path : post;
    if (typeof path !== "string") {
      throw new Error(name + " needs a post: one of your objects, or its path");
    }
    return path;
  };
  const emptyPage = async (post) => {
    postPath(post);
    return { items: [], total: 0, next: null };
  };
  return Object.freeze({
    counts: async (posts) => {
      if (!Array.isArray(posts)) {
        throw new Error(name + ".counts takes a list of posts");
      }
      return posts.map((post) => {
        postPath(post);
        return { likes: 0, reposts: 0, replies: 0, mentions: 0, bookmarks: 0 };
      });
    },
    replies: emptyPage,
    reactions: emptyPage,
  });
};

/**
 * What a local run sells: the file `ARCHIVAL_PAYMENTS` names, in the shape a
 * deployed carrier's `site.payments` answers (`account`, and `catalog.items`
 * holding each product with its variants), which `archival build --payments`
 * reads too. Without one, or with `"account": null`, a local run reads as a
 * site with no payment account connected. With an account, the run also serves
 * the cart a site with a payment account gets: its script at
 * /_archival/cart.js, and the checkout that script posts to at
 * /_archival/checkout. Read again whenever it changes, so stock a test took is
 * put back by saving the file.
 */
let paymentsFile = { path: null, mtimeMs: 0, account: null, products: [] };

const NO_PAYMENTS = Object.freeze({ account: null, products: [] });

const isRecord = (value) =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** Refuses a file `archival build --payments` would refuse. */
const checkPaymentsFile = (file, parsed) => {
  const refuse = (message) =>
    new Error(`${file} is not a payments file: ${message}`);
  if (!isRecord(parsed)) {
    throw refuse("it must be a JSON object");
  }
  if (parsed.account != null && !isRecord(parsed.account)) {
    throw refuse("account must be an object, or null for no account");
  }
  const items = parsed.catalog?.items ?? [];
  if (!Array.isArray(items)) {
    throw refuse("catalog.items must be a list of products");
  }
  for (const [i, product] of items.entries()) {
    if (
      !isRecord(product) ||
      typeof product.id !== "string" ||
      typeof product.name !== "string" ||
      !Array.isArray(product.variants)
    ) {
      throw refuse(`catalog.items[${i}] needs an id, a name and its variants`);
    }
    for (const [j, variant] of product.variants.entries()) {
      if (
        !isRecord(variant) ||
        typeof variant.id !== "string" ||
        !(
          variant.amount === null ||
          (Number.isSafeInteger(variant.amount) && variant.amount >= 0)
        ) ||
        typeof variant.currency !== "string"
      ) {
        throw refuse(
          `catalog.items[${i}].variants[${j}] needs an id, an amount (null for no fixed price) and a currency`,
        );
      }
    }
  }
};

/**
 * Whether a variant can be bought, as a provider decides it: priced, in stock
 * or uncounted, and not taken off sale by the file's `available: false`.
 */
const isAvailable = (onSale, variant) =>
  onSale &&
  variant.amount !== null &&
  (variant.stock === null || variant.stock > 0);

const localPayments = () => {
  const file = process.env.ARCHIVAL_PAYMENTS;
  if (!file) {
    return NO_PAYMENTS;
  }
  let mtimeMs;
  try {
    mtimeMs = statSync(file).mtimeMs;
  } catch {
    throw new Error(`ARCHIVAL_PAYMENTS names ${file}, which cannot be read`);
  }
  if (paymentsFile.path !== file || paymentsFile.mtimeMs !== mtimeMs) {
    let parsed;
    try {
      parsed = JSON.parse(readFileSync(file, "utf-8"));
    } catch (e) {
      throw new Error(`${file} is not JSON: ${e.message}`);
    }
    checkPaymentsFile(file, parsed);
    if (parsed.account === null) {
      paymentsFile = { path: file, mtimeMs, ...NO_PAYMENTS };
      return paymentsFile;
    }
    const products = [...(parsed.catalog?.items ?? [])].sort((a, b) =>
      a.id < b.id ? -1 : a.id > b.id ? 1 : 0,
    );
    const currency =
      products.flatMap((product) => product.variants)[0]?.currency ?? "usd";
    paymentsFile = {
      path: file,
      mtimeMs,
      account: {
        provider: "stripe",
        name: "Local payments",
        livemode: false,
        currency,
        ...(parsed.account ?? {}),
      },
      products: products.map((product) => ({
        description: null,
        images: [],
        metadata: {},
        ...product,
        variants: product.variants.map(({ available = true, ...given }) => {
          const variant = {
            name: null,
            sku: null,
            recurring: null,
            stock: null,
            ...given,
            product: product.id,
          };
          return { ...variant, available: isAvailable(available, variant) };
        }),
      })),
    };
  }
  return paymentsFile;
};

/** id -> a checkout a carrier started, kept until this process exits. */
const checkouts = new Map();

const CHECKOUT_PATH = /^\/__payments\/(co_[A-Za-z0-9_-]{22})(\/pay|\/cancel)?$/;

const checkoutUrl = (id) => {
  const address = server.listening ? server.address() : null;
  return address ? `http://127.0.0.1:${address.port}/__payments/${id}` : null;
};

// The limits a deployed site's payments are held to.
const PAYMENTS_DEFAULT_PAGE = 20;
const PAYMENTS_MAX_PAGE = 100;
const PAYMENTS_MAX_IDS = 100;
const CHECKOUT_MAX_ITEMS = 100;
const CHECKOUT_MAX_QUANTITY = 10_000;
const CHECKOUT_MAX_NAME_LENGTH = 250;
const CHECKOUT_MAX_URL_LENGTH = 2_000;
const CHECKOUT_MAX_EMAIL_LENGTH = 320;
const CHECKOUT_MAX_METADATA_KEYS = 20;
const CHECKOUT_MAX_METADATA_KEY_LENGTH = 40;
const CHECKOUT_MAX_METADATA_VALUE_LENGTH = 500;
const CHECKOUT_MAX_COUNTRIES = 250;
const CHECKOUT_MAX_IDEMPOTENCY_KEY_LENGTH = 200;

const NOT_CONNECTED = Object.freeze({
  ok: false,
  status: 409,
  error: "This site has no payment provider connected.",
});

const invalid = (error) => ({ ok: false, status: 400, error });

const wholeNumber = (value, max) =>
  Number.isInteger(value) && value >= 1 && value <= max;

const optionalString = (value, field, max) => {
  if (value === undefined || value === null) {
    return { ok: true, value: null };
  }
  return typeof value === "string" && value && value.length <= max
    ? { ok: true, value }
    : invalid(`${field} must be a string of at most ${max} characters`);
};

const parseIds = (ids) => {
  if (
    !Array.isArray(ids) ||
    ids.some((id) => typeof id !== "string" || !id || id.length > 255)
  ) {
    return invalid("ids must be a list of ids");
  }
  if (ids.length > PAYMENTS_MAX_IDS) {
    return invalid(`At most ${PAYMENTS_MAX_IDS} ids can be looked up at once`);
  }
  return { ok: true, ids };
};

const parseItems = (items) => {
  if (!Array.isArray(items) || items.length === 0) {
    return invalid("items must be a non-empty list");
  }
  if (items.length > CHECKOUT_MAX_ITEMS) {
    return invalid(`A checkout takes at most ${CHECKOUT_MAX_ITEMS} items`);
  }
  const parsed = [];
  for (const [i, item] of items.entries()) {
    if (!isRecord(item)) {
      return invalid(`items[${i}] must be an object`);
    }
    if (!wholeNumber(item.quantity, CHECKOUT_MAX_QUANTITY)) {
      return invalid(
        `items[${i}].quantity must be a whole number from 1 to ${CHECKOUT_MAX_QUANTITY}`,
      );
    }
    if (typeof item.variant === "string" && item.variant) {
      parsed.push({ variant: item.variant, quantity: item.quantity });
      continue;
    }
    if (
      typeof item.name !== "string" ||
      !item.name.trim() ||
      item.name.length > CHECKOUT_MAX_NAME_LENGTH
    ) {
      return invalid(
        `items[${i}] needs a variant, or a name of at most ${CHECKOUT_MAX_NAME_LENGTH} characters and an amount`,
      );
    }
    if (!wholeNumber(item.amount, Number.MAX_SAFE_INTEGER)) {
      return invalid(
        `items[${i}].amount must be a positive whole number in the currency's smallest unit`,
      );
    }
    parsed.push({
      name: item.name.trim(),
      amount: item.amount,
      quantity: item.quantity,
    });
  }
  return { ok: true, items: parsed };
};

const parseUrl = (value, field) =>
  typeof value === "string" &&
  value.length <= CHECKOUT_MAX_URL_LENGTH &&
  URL.canParse(value)
    ? { ok: true, url: value }
    : invalid(`${field} must be a url`);

const parseMetadata = (value) => {
  if (value === undefined || value === null) {
    return { ok: true, metadata: {} };
  }
  if (!isRecord(value)) {
    return invalid("metadata must be an object of strings");
  }
  const entries = Object.entries(value);
  if (
    entries.length > CHECKOUT_MAX_METADATA_KEYS ||
    entries.some(
      ([key, entry]) =>
        !key ||
        key.length > CHECKOUT_MAX_METADATA_KEY_LENGTH ||
        typeof entry !== "string" ||
        entry.length > CHECKOUT_MAX_METADATA_VALUE_LENGTH,
    )
  ) {
    return invalid(
      `metadata takes at most ${CHECKOUT_MAX_METADATA_KEYS} strings of at most ${CHECKOUT_MAX_METADATA_VALUE_LENGTH} characters, under keys of at most ${CHECKOUT_MAX_METADATA_KEY_LENGTH}`,
    );
  }
  return { ok: true, metadata: value };
};

const parseShipping = (value) => {
  if (value === undefined || value === null) {
    return { ok: true, shipping: null };
  }
  if (!isRecord(value)) {
    return invalid("shipping must be an object");
  }
  const countries = value.countries ?? [];
  if (
    !Array.isArray(countries) ||
    countries.length > CHECKOUT_MAX_COUNTRIES ||
    countries.some(
      (country) =>
        typeof country !== "string" || !/^[A-Za-z]{2}$/.test(country),
    )
  ) {
    return invalid(
      "shipping.countries must be a list of two-letter country codes",
    );
  }
  return {
    ok: true,
    shipping: { countries: countries.map((country) => country.toUpperCase()) },
  };
};

/** A checkout request as a deployed site parses it, or why it refuses it. */
const parseCheckout = (body) => {
  const items = parseItems(body.items);
  if (!items.ok) return items;
  const successUrl = parseUrl(body.successUrl, "successUrl");
  if (!successUrl.ok) return successUrl;
  const cancelUrl =
    body.cancelUrl === undefined || body.cancelUrl === null
      ? null
      : parseUrl(body.cancelUrl, "cancelUrl");
  if (cancelUrl && !cancelUrl.ok) return cancelUrl;
  const customerEmail = optionalString(
    body.customerEmail,
    "customerEmail",
    CHECKOUT_MAX_EMAIL_LENGTH,
  );
  if (!customerEmail.ok) return customerEmail;
  const metadata = parseMetadata(body.metadata);
  if (!metadata.ok) return metadata;
  const shipping = parseShipping(body.shipping);
  if (!shipping.ok) return shipping;
  const idempotencyKey = optionalString(
    body.idempotencyKey,
    "idempotencyKey",
    CHECKOUT_MAX_IDEMPOTENCY_KEY_LENGTH,
  );
  if (!idempotencyKey.ok) return idempotencyKey;
  return {
    ok: true,
    checkout: {
      items: items.items,
      successUrl: successUrl.url,
      cancelUrl: cancelUrl ? cancelUrl.url : null,
      customerEmail: customerEmail.value,
      metadata: metadata.metadata,
      shipping: shipping.shipping,
      idempotencyKey: idempotencyKey.value,
    },
  };
};

const LOOPBACK_HOST = /^(localhost|127\.0\.0\.1|\[::1\])$|\.localhost$/;

/**
 * Whether a buyer sent to `value` lands on this site: a deployed site takes
 * only its own domains, and a local one its own address or this machine.
 */
const onLocalSite = (value, siteUrl) => {
  const url = new URL(value);
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    return false;
  }
  const site = URL.canParse(siteUrl ?? "") ? new URL(siteUrl) : null;
  return url.origin === site?.origin || LOOPBACK_HOST.test(url.hostname);
};

const localVariant = (id) =>
  localPayments()
    .products.flatMap((product) => product.variants)
    .find((variant) => variant.id === id) ?? null;

/**
 * Starts a checkout `by` asked for over the local payments, or says why a
 * deployed site refuses it, with the status its own checkout answers that
 * with. `request` is what `parseCheckout` made of the call.
 */
const createCheckout = (request, by) => {
  const { account, products } = localPayments();
  if (!account) {
    return NOT_CONNECTED;
  }
  if (request.idempotencyKey) {
    const made = [...checkouts.values()].find(
      (other) => other.idempotencyKey === request.idempotencyKey,
    );
    if (made) {
      return { ok: true, id: made.id, url: checkoutUrl(made.id) };
    }
  }
  const lines = [];
  const short = [];
  let currency = account.currency;
  for (const item of request.items) {
    if (!("variant" in item)) {
      lines.push({ variant: null, currency: null, ...item });
      continue;
    }
    const variant = localVariant(item.variant);
    if (!variant) {
      return {
        ok: false,
        status: 422,
        error: `${item.variant} is not a variant this site's payment account sells`,
      };
    }
    if (variant.amount === null) {
      return {
        ok: false,
        status: 422,
        error: `${item.variant} has no fixed price, so it cannot be put in a checkout`,
      };
    }
    if (
      !variant.available ||
      (variant.stock !== null && variant.stock < item.quantity)
    ) {
      short.push(variant.id);
    }
    currency ??= variant.currency;
    lines.push({
      variant: variant.id,
      name: [
        products.find((product) => product.id === variant.product)?.name,
        variant.name,
      ]
        .filter(Boolean)
        .join(" - "),
      amount: variant.amount,
      currency: variant.currency,
      quantity: item.quantity,
    });
  }
  if (short.length) {
    return { ok: false, status: 409, error: "out_of_stock", outOfStock: short };
  }
  if (!currency) {
    return {
      ok: false,
      status: 422,
      error:
        "This site's payment account names no currency, so a checkout needs at least one variant to be priced in",
    };
  }
  for (const line of lines) {
    line.currency ??= currency;
  }
  const id = `co_${randomBytes(16).toString("base64url")}`;
  const successUrl = new URL(request.successUrl);
  successUrl.searchParams.set("checkout", id);
  checkouts.set(id, {
    id,
    status: "open",
    lines,
    currency,
    successUrl: successUrl.href,
    cancelUrl: request.cancelUrl,
    customerEmail: request.customerEmail,
    idempotencyKey: request.idempotencyKey,
  });
  const url = checkoutUrl(id);
  console.log(`[carrier] ${by} started a checkout: ${url}`);
  return { ok: true, id, url };
};

/**
 * Builds the payments facade, which the carrier calls `name`. Every call is
 * checked and refused as a deployed one is; lookups answer from the local
 * file, and a checkout opens as a page of this process with a button that
 * pays it and one that backs out, instead of at a payment provider.
 */
const makePayments = (name, siteUrl) => {
  const fail = (method, refusal) => {
    const error = new Error(`${name}.${method} failed: ${refusal.error}`);
    if (refusal.outOfStock) {
      error.outOfStock = refusal.outOfStock;
    }
    return error;
  };
  const lookup = (method, find) => async (ids) => {
    if (!Array.isArray(ids) || ids.some((id) => typeof id !== "string")) {
      throw new Error(`${name}.${method} takes a list of ids`);
    }
    const parsed = parseIds(ids);
    if (!parsed.ok) {
      throw fail(method, parsed);
    }
    return parsed.ids.map(find);
  };
  return Object.freeze({
    account: async () => localPayments().account,
    catalog: async (page) => {
      const limit = page?.limit ?? PAYMENTS_DEFAULT_PAGE;
      if (!wholeNumber(limit, PAYMENTS_MAX_PAGE)) {
        throw fail(
          "catalog",
          invalid(`limit must be a whole number from 1 to ${PAYMENTS_MAX_PAGE}`),
        );
      }
      const cursor = optionalString(page?.cursor || null, "cursor", 255);
      if (!cursor.ok) {
        throw fail("catalog", cursor);
      }
      const { products } = localPayments();
      const start = cursor.value
        ? products.findIndex((product) => product.id > cursor.value)
        : 0;
      const items = start < 0 ? [] : products.slice(start, start + limit);
      const more = start >= 0 && start + limit < products.length;
      return {
        items,
        total: products.length,
        next: more ? items.at(-1).id : null,
      };
    },
    products: lookup(
      "products",
      (id) =>
        localPayments().products.find((product) => product.id === id) ?? null,
    ),
    variants: lookup("variants", localVariant),
    checkout: Object.freeze({
      create: async (checkout) => {
        if (!checkout || typeof checkout !== "object") {
          throw new Error(`${name}.checkout.create takes the checkout to start`);
        }
        const parsed = parseCheckout(checkout);
        if (!parsed.ok) {
          throw fail("checkout.create", parsed);
        }
        const { successUrl, cancelUrl } = parsed.checkout;
        if (
          !onLocalSite(successUrl, siteUrl) ||
          (cancelUrl !== null && !onLocalSite(cancelUrl, siteUrl))
        ) {
          throw fail(
            "checkout.create",
            invalid(
              "successUrl and cancelUrl must be urls on this site, as site.url builds them",
            ),
          );
        }
        const made = createCheckout(parsed.checkout, name);
        if (!made.ok) {
          throw fail("checkout.create", made);
        }
        return { id: made.id, url: made.url };
      },
      get: async (id) => {
        if (typeof id !== "string" || !id || id.length > 255) {
          throw fail("checkout.get", invalid("id must be a checkout's id"));
        }
        if (!localPayments().account) {
          throw fail("checkout.get", NOT_CONNECTED);
        }
        const checkout = checkouts.get(id);
        if (!checkout) {
          throw fail("checkout.get", {
            error: "That is not a checkout this site made",
          });
        }
        return {
          id,
          status: checkout.status,
          amountTotal: checkoutTotal(checkout),
          currency: checkout.currency,
          customerEmail: checkout.customerEmail,
        };
      },
    }),
  });
};

const checkoutTotal = (checkout) =>
  checkout.lines.reduce((sum, line) => sum + line.amount * line.quantity, 0);

const money = (amount, currency) => {
  try {
    const format = new Intl.NumberFormat("en", {
      style: "currency",
      currency: currency.toUpperCase(),
    });
    const digits = format.resolvedOptions().maximumFractionDigits;
    return format.format(amount / 10 ** digits);
  } catch {
    return `${amount} ${currency}`;
  }
};

/** The page that stands in for a provider's checkout in a local run. */
export const checkoutPage = (checkout) => `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Local checkout</title>
<style>
  body { font: 16px/1.5 system-ui, sans-serif; max-width: 32rem; margin: 3rem auto; padding: 0 1rem; color: #1a1a1a; }
  .note { background: #fff7d6; border: 1px solid #e6d27a; border-radius: 6px; padding: .75rem 1rem; }
  table { width: 100%; border-collapse: collapse; margin: 1.5rem 0; }
  td { padding: .4rem 0; border-bottom: 1px solid #e5e5e5; }
  td:last-child { text-align: right; }
  .total td { font-weight: 600; border-bottom: 0; }
  .actions { display: flex; gap: .75rem; }
  a.button { padding: .6rem 1.2rem; border-radius: 6px; text-decoration: none; border: 1px solid #1a1a1a; color: #1a1a1a; }
  a.pay { background: #1a1a1a; color: #fff; }
</style>
</head>
<body>
<h1>Local checkout</h1>
<p class="note">This page stands in for your payment provider while <code>archival run</code> is running. Nothing is charged.</p>
${
  checkout.status === "open"
    ? ""
    : `<p>This checkout is ${escapeHtml(checkout.status)}.</p>`
}
<table>
${checkout.lines
  .map(
    (line) =>
      `<tr><td>${escapeHtml(line.name)} &times; ${line.quantity}</td><td>${escapeHtml(money(line.amount * line.quantity, line.currency))}</td></tr>`,
  )
  .join("\n")}
<tr class="total"><td>Total</td><td>${escapeHtml(money(checkoutTotal(checkout), checkout.currency))}</td></tr>
</table>
${
  checkout.status === "open"
    ? `<div class="actions"><a class="button pay" href="/__payments/${checkout.id}/pay">Pay</a>${
        checkout.cancelUrl
          ? `<a class="button" href="/__payments/${checkout.id}/cancel">Cancel</a>`
          : ""
      }</div>`
    : ""
}
</body>
</html>
`;

/** Answers the local checkout page, and what its two buttons do. */
export const checkoutResponse = (id, action) => {
  const checkout = checkouts.get(id);
  if (!checkout) {
    return new Response(
      "This checkout is gone: it was kept only while archival run was running.",
      { status: 404, headers: { "content-type": "text/plain; charset=utf-8" } },
    );
  }
  const redirect = (location) =>
    new Response(null, { status: 302, headers: { location } });
  if (action === "/cancel" && checkout.cancelUrl) {
    return redirect(checkout.cancelUrl);
  }
  if (action === "/pay") {
    if (checkout.status === "open") {
      checkout.status = "paid";
      for (const line of checkout.lines) {
        const variant = localVariant(line.variant);
        if (variant && variant.stock != null) {
          variant.stock = Math.max(0, variant.stock - line.quantity);
          variant.available &&= variant.stock > 0;
        }
      }
    }
    return redirect(checkout.successUrl);
  }
  return new Response(checkoutPage(checkout), {
    headers: {
      "content-type": "text/html; charset=utf-8",
      "content-security-policy":
        "default-src 'none'; style-src 'unsafe-inline'",
      "cache-control": "no-store",
    },
  });
};

const CART_SCRIPT_PATH = "/_archival/cart.js";
const CART_CHECKOUT_PATH = "/_archival/checkout";
/** The site as the browser that made a request addresses it. */
const siteOrigin = (headers) => {
  const host = headers["x-forwarded-host"] ?? headers.host;
  const origin = URL.canParse(headers.origin ?? "")
    ? new URL(headers.origin)
    : null;
  return origin?.host === host ? origin.origin : `http://${host}`;
};

const onSite = (value, origin) => {
  if (!URL.canParse(value, origin)) return null;
  const url = new URL(value, origin);
  return url.origin === origin ? url : null;
};

/** The page the buyer left, without the checkout an earlier return left on it. */
const pageLeft = (referer, origin) => {
  const page = (referer && onSite(referer, origin)) || new URL("/", origin);
  page.searchParams.delete("checkout");
  page.hash = "";
  return page.href;
};

/**
 * The checkout a site with a payment account answers for a cart that names no
 * carrier. Anyone can post to it, so it sells only the account's own variants
 * at the account's prices, and returns the buyer to a path on the site: the
 * page they left, unless the body names another.
 */
const cartCheckout = (posted, origin, referer) => {
  const refuse = (status, error) => Response.json({ error }, { status });
  let body = null;
  try {
    body = JSON.parse(posted.toString("utf-8"));
  } catch {
    // Refused below, as the body it is.
  }
  if (!isRecord(body)) {
    return refuse(400, "A checkout is a JSON object with items");
  }
  if (
    Array.isArray(body.items) &&
    body.items.some(
      (item) =>
        !isRecord(item) || typeof item.variant !== "string" || !item.variant,
    )
  ) {
    return refuse(400, "Every item needs the id of a variant the site sells");
  }
  const urls = {
    successUrl: pageLeft(referer, origin),
    cancelUrl: pageLeft(referer, origin),
  };
  for (const field of ["successUrl", "cancelUrl"]) {
    const sitePath = body[field];
    if (sitePath == null) continue;
    const url =
      typeof sitePath === "string" && sitePath.startsWith("/")
        ? onSite(sitePath, origin)
        : null;
    if (!url) {
      return refuse(400, `${field} must be a path on this site`);
    }
    urls[field] = url.href;
  }
  const parsed = parseCheckout({
    items: body.items,
    shipping: body.shipping,
    ...urls,
  });
  if (!parsed.ok) {
    return refuse(parsed.status, parsed.error);
  }
  const made = createCheckout(parsed.checkout, "The cart");
  if (!made.ok) {
    const { ok, status, ...refusal } = made;
    return Response.json(refusal, { status });
  }
  return Response.json({ url: made.url, checkout: made.id });
};

/**
 * Answers what a site with a payment account serves for its cart: the script
 * that sets `window.archivalCart`, and the checkout that script posts its
 * lines to when the page names no carrier. Null for anything else, and for a
 * run with no payment account, where both paths are the site's own.
 */
export const cartResponse = (pathname, method, headers, body) => {
  if (pathname !== CART_SCRIPT_PATH && pathname !== CART_CHECKOUT_PATH) {
    return null;
  }
  if (!localPayments().account) {
    return null;
  }
  if (pathname === CART_CHECKOUT_PATH) {
    return method === "POST"
      ? cartCheckout(body, siteOrigin(headers), headers.referer ?? null)
      : Response.json(
          { error: "method_not_allowed" },
          { status: 405, headers: { allow: "POST" } },
        );
  }
  if (method !== "GET" && method !== "HEAD") {
    return new Response(null, { status: 405, headers: { allow: "GET, HEAD" } });
  }
  return new Response(
    method === "HEAD" ? null : readFileSync(path.join(HERE, "cart.js")),
    {
      headers: {
        "content-type": "text/javascript; charset=utf-8",
        "cache-control": "no-cache",
      },
    },
  );
};

const CARRIER_CALLS = new Map([
  [
    1,
    // UPLOADS has methods, so unlike the rest of the vars it cannot be part of
    // the pushed payload. It wins over a site object of the same name, which is
    // how every injected var resolves that collision.
    (carrier, params, body, current) =>
      carrier(
        params,
        body,
        Object.freeze({
          ...current.objects,
          SITE_URL: current.siteUrl,
          UPLOADS: makeUploads("objects.UPLOADS", current),
        }),
      ),
  ],
  [
    2,
    (carrier, params, body, current, name) =>
      carrier(
        params,
        body,
        current.objects,
        Object.freeze({
          url: current.siteUrl,
          uploads: makeUploads("site.uploads", current),
          email: makeEmail("site.email", name),
          sql: makeSql("site.sql", current.database),
          activitypub: makeActivityPub("site.activitypub"),
          webmentions: makeWebmentions("site.webmentions"),
          atproto: makeATProto("site.atproto"),
          interactions: makeInteractions("site.interactions"),
          payments: makePayments("site.payments", current.siteUrl),
        }),
      ),
  ],
]);

/**
 * The carrier API version a carrier's package.json text asks for. One that
 * names none is version 1 and always will be, so a carrier that never names
 * one keeps the calling convention it was written for.
 */
export const carrierApiVersion = (packageJson) => {
  if (packageJson === undefined) {
    return 1;
  }
  let manifest;
  try {
    manifest = JSON.parse(packageJson);
  } catch (e) {
    throw new Error(`package.json is not valid JSON: ${e.message}`);
  }
  const version = manifest?.archival?.carrier;
  if (version === undefined) {
    return 1;
  }
  if (!CARRIER_CALLS.has(version)) {
    throw new Error(
      `package.json asks for carrier API ${JSON.stringify(version)}; the versions that exist are ${[...CARRIER_CALLS.keys()].join(", ")}`,
    );
  }
  return version;
};

/**
 * Calls a carrier `buildAndLoad` answered with a request, against `current`.
 * `name` is the carrier's directory, which an email it sends names.
 */
export const invokeCarrier = (
  { carrier, api },
  params,
  body,
  current,
  name = "carrier",
) => CARRIER_CALLS.get(api)(carrier, params, body, current, name);

/** An email a carrier sent while this process has run, as its page serves it. */
export const sentEmail = (id) => emails.get(id);

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
 * the function it default-exports and the carrier API version it names.
 */
export const buildAndLoad = async (name, root, out) => {
  const files = readCarrierFiles(root);
  const api = carrierApiVersion(files.get("package.json"));
  const compiled = await buildCarrier({
    files,
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
  return { carrier, api };
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
  let loaded;
  try {
    loaded = await loadCarrier(name);
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
    return toResponse(
      await invokeCarrier(loaded, params, parsed, state, name),
    );
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
    const email = EMAIL_PATH.exec(path);
    if (email && req.method === "GET") {
      return send(res, emailResponse(email[1]));
    }
    const checkout = CHECKOUT_PATH.exec(path);
    if (checkout && req.method === "GET") {
      return send(res, checkoutResponse(checkout[1], checkout[2]));
    }
    if (path === CART_SCRIPT_PATH || path === CART_CHECKOUT_PATH) {
      const cart = cartResponse(
        path,
        req.method,
        req.headers,
        await readBody(req),
      );
      if (cart) {
        return send(res, cart);
      }
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
 * Node warns that its type stripper and its SQLite module are experimental the
 * first time a TypeScript carrier is built or site.sql is used. Every other
 * warning is the carrier author's to read, so only those are dropped.
 */
const quietHarnessWarnings = () => {
  const [print] = process.listeners("warning");
  process.removeAllListeners("warning");
  process.on("warning", (warning) => {
    if (
      warning.name === "ExperimentalWarning" &&
      /stripTypeScriptTypes|SQLite/.test(warning.message)
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
  quietHarnessWarnings();
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
