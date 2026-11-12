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
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import {
  buildAndLoad,
  carrierApiVersion,
  cartResponse,
  checkoutResponse,
  closeDatabases,
  emailPage,
  invokeCarrier,
  parseRequestBody,
  readCarrierFiles,
  sentEmail,
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
  assert.equal(typeof site.email.send, "function");
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
  assert.equal(
    await site.activitypub.setLocalLikes({ path: "artist/a" }, 3),
    undefined,
  );
  await assert.rejects(
    site.activitypub.setLocalLikes("artist/a", -1),
    /site\.activitypub\.setLocalLikes needs a whole number of likes/,
  );
  assert.deepEqual(await site.webmentions.counts([{ path: "artist/a" }, ""]), [
    { replies: 0, likes: 0, reposts: 0, bookmarks: 0, mentions: 0 },
    { replies: 0, likes: 0, reposts: 0, bookmarks: 0, mentions: 0 },
  ]);
  assert.deepEqual(await site.webmentions.list("artist/a", { kind: "reply" }), {
    items: [],
    total: 0,
    next: null,
  });
  await assert.rejects(
    site.webmentions.list({ name: "no path" }),
    /site\.webmentions needs a page/,
  );
  assert.equal(await site.atproto.account(), null);
  assert.deepEqual(await site.atproto.counts([{ path: "artist/a" }]), [
    { likes: 0, reposts: 0, replies: 0 },
  ]);
  assert.deepEqual(await site.atproto.replies({ path: "artist/a" }), {
    items: [],
    total: 0,
    next: null,
  });
  await assert.rejects(
    site.atproto.reposts({ name: "no path" }),
    /site\.atproto needs a post/,
  );
  assert.deepEqual(await site.interactions.counts([{ path: "artist/a" }]), [
    { likes: 0, reposts: 0, replies: 0, mentions: 0, bookmarks: 0 },
  ]);
  assert.deepEqual(await site.interactions.replies("artist/a"), {
    items: [],
    total: 0,
    next: null,
  });
  assert.deepEqual(await site.interactions.reactions({ path: "artist/a" }), {
    items: [],
    total: 0,
    next: null,
  });
  await assert.rejects(
    site.interactions.reactions({ name: "no path" }),
    /site\.interactions needs a post/,
  );
  assert.equal(await site.payments.account(), null);
  assert.deepEqual(await site.payments.catalog(), {
    items: [],
    total: 0,
    next: null,
  });
  assert.deepEqual(await site.payments.variants(["price_a", "price_b"]), [
    null,
    null,
  ]);
  await assert.rejects(
    site.payments.products("prod_a"),
    /site\.payments\.products takes a list of ids/,
  );
  await assert.rejects(
    site.payments.checkout.create({
      items: [{ name: "Tip", amount: 100, quantity: 1 }],
      successUrl: "http://localhost:1234/thanks",
    }),
    /site\.payments\.checkout\.create failed: This site has no payment provider connected/,
  );
});

const paymentsSite = async (t, catalog) => {
  const dir = mkdtempSync(path.join(tmpdir(), "harness-payments-"));
  const file = path.join(dir, "payments.json");
  writeFileSync(file, JSON.stringify(catalog));
  process.env.ARCHIVAL_PAYMENTS = file;
  t.after(() => {
    delete process.env.ARCHIVAL_PAYMENTS;
    rmSync(dir, { recursive: true, force: true });
  });
  const [, , , site] = await invokeCarrier(
    { carrier: (...args) => args, api: 2 },
    new URLSearchParams(),
    null,
    { objects: {}, siteUrl: "http://localhost:1234" },
  );
  return site;
};

const PAYMENTS = {
  account: { name: "Wildflower Press", currency: "usd" },
  catalog: {
    items: [
      {
        id: "prod_b",
        name: "Print",
        variants: [{ id: "price_print", amount: 4500, currency: "usd" }],
      },
      {
        id: "prod_a",
        name: "Notebook",
        variants: [
          { id: "price_pocket", name: "Pocket", amount: 1200, currency: "usd" },
          {
            id: "price_last",
            name: "Last one",
            amount: 900,
            currency: "usd",
            stock: 1,
          },
          { id: "price_custom", amount: null, currency: "usd" },
        ],
      },
    ],
  },
};

test("site.payments sells what the local payments file holds", async (t) => {
  const site = await paymentsSite(t, PAYMENTS);
  assert.deepEqual(await site.payments.account(), {
    provider: "stripe",
    name: "Wildflower Press",
    livemode: false,
    currency: "usd",
  });
  const first = await site.payments.catalog({ limit: 1 });
  assert.equal(first.total, 2);
  assert.deepEqual(
    first.items.map(({ id }) => id),
    ["prod_a"],
  );
  assert.equal(first.next, "prod_a");
  const second = await site.payments.catalog({ limit: 1, cursor: first.next });
  assert.deepEqual(
    second.items.map(({ id }) => id),
    ["prod_b"],
  );
  assert.equal(second.next, null);
  const [pocket, gone] = await site.payments.variants([
    "price_pocket",
    "price_gone",
  ]);
  assert.deepEqual(pocket, {
    id: "price_pocket",
    product: "prod_a",
    name: "Pocket",
    sku: null,
    amount: 1200,
    currency: "usd",
    recurring: null,
    stock: null,
    available: true,
  });
  assert.equal(gone, null);
  const [print] = await site.payments.products(["prod_b"]);
  assert.equal(print.name, "Print");
  assert.deepEqual(print.images, []);
});

test("a local checkout is refused as a deployed one is", async (t) => {
  const site = await paymentsSite(t, PAYMENTS);
  const create = (checkout) =>
    site.payments.checkout.create({
      successUrl: "http://localhost:1234/thanks",
      ...checkout,
    });
  await assert.rejects(create({ items: [] }), /items must be a non-empty list/);
  await assert.rejects(
    create({ items: [{ variant: "price_gone", quantity: 1 }] }),
    /price_gone is not a variant/,
  );
  await assert.rejects(
    create({ items: [{ variant: "price_custom", quantity: 1 }] }),
    /no fixed price/,
  );
  await assert.rejects(
    create({ items: [{ variant: "price_pocket", quantity: 0 }] }),
    /quantity must be a whole number/,
  );
  await assert.rejects(
    create({
      items: [{ variant: "price_pocket", quantity: 1 }],
      successUrl: "not a url",
    }),
    /successUrl must be a url/,
  );
  await assert.rejects(
    create({ items: [{ variant: "price_last", quantity: 2 }] }),
    (error) => {
      assert.match(error.message, /checkout\.create failed: out_of_stock/);
      assert.deepEqual(error.outOfStock, ["price_last"]);
      return true;
    },
  );
});

test("a local checkout is paid on a page of the harness, which takes its stock", async (t) => {
  const site = await paymentsSite(t, PAYMENTS);
  const made = await site.payments.checkout.create({
    items: [
      { variant: "price_last", quantity: 1 },
      { name: "Gift wrap", amount: 300, quantity: 2 },
    ],
    successUrl: "http://localhost:1234/thanks?from=shop",
    cancelUrl: "http://localhost:1234/cart",
    idempotencyKey: "cart-1",
  });
  assert.match(made.id, /^co_/);
  assert.deepEqual(
    await site.payments.checkout.create({
      items: [{ variant: "price_pocket", quantity: 1 }],
      successUrl: "http://localhost:1234/thanks",
      idempotencyKey: "cart-1",
    }),
    made,
  );
  assert.deepEqual(await site.payments.checkout.get(made.id), {
    id: made.id,
    status: "open",
    amountTotal: 1500,
    currency: "usd",
    customerEmail: null,
  });

  const page = await checkoutResponse(made.id).text();
  assert.match(page, /Notebook - Last one &times; 1/);
  assert.match(page, /Gift wrap &times; 2/);
  assert.match(page, /\$15\.00/);
  assert.match(page, new RegExp(`/__payments/${made.id}/pay`));

  const cancelled = checkoutResponse(made.id, "/cancel");
  assert.equal(cancelled.headers.get("location"), "http://localhost:1234/cart");
  assert.equal((await site.payments.checkout.get(made.id)).status, "open");

  const paid = checkoutResponse(made.id, "/pay");
  assert.equal(paid.status, 302);
  assert.equal(
    paid.headers.get("location"),
    `http://localhost:1234/thanks?from=shop&checkout=${made.id}`,
  );
  assert.equal((await site.payments.checkout.get(made.id)).status, "paid");
  const [last] = await site.payments.variants(["price_last"]);
  assert.equal(last.stock, 0);
  assert.equal(last.available, false);
  assert.doesNotMatch(await checkoutResponse(made.id).text(), /\/pay"/);

  assert.equal(checkoutResponse("co_" + "0".repeat(36)).status, 404);
  await assert.rejects(
    site.payments.checkout.get("co_unknown"),
    /not a checkout this site made/,
  );
});

const SHOP = "http://localhost:1234";

const cartCheckout = (body, headers = {}) =>
  cartResponse(
    "/_archival/checkout",
    "POST",
    { "x-forwarded-host": "localhost:1234", origin: SHOP, ...headers },
    Buffer.from(typeof body === "string" ? body : JSON.stringify(body)),
  );

test("a run with no payment account leaves the cart's paths to the site", () => {
  assert.equal(cartResponse("/_archival/cart.js", "GET", {}, null), null);
  assert.equal(cartCheckout({ items: [] }), null);
});

test("a run with a payment account serves the cart script", async (t) => {
  await paymentsSite(t, PAYMENTS);
  const script = cartResponse("/_archival/cart.js", "GET", {}, null);
  assert.equal(script.status, 200);
  assert.equal(
    script.headers.get("content-type"),
    "text/javascript; charset=utf-8",
  );
  const source = await script.text();
  assert.equal(
    source,
    readFileSync(new URL("./cart.js", import.meta.url), "utf-8"),
  );
  const window = {
    localStorage: { getItem: () => null },
    location: { search: "" },
    addEventListener: () => {},
  };
  new Function("window", source)(window);
  assert.deepEqual(window.archivalCart.items(), []);

  const head = cartResponse("/_archival/cart.js", "HEAD", {}, null);
  assert.equal(head.status, 200);
  assert.equal(await head.text(), "");
  assert.equal(cartResponse("/_archival/cart.js", "POST", {}, null).status, 405);
  assert.equal(cartResponse("/_archival/cart", "GET", {}, null), null);
});

test("the cart's checkout returns the buyer to the page they left", async (t) => {
  const site = await paymentsSite(t, PAYMENTS);
  const answer = cartCheckout(
    { items: [{ variant: "price_last", quantity: 1 }] },
    { referer: `${SHOP}/shop?size=a5&checkout=co_earlier#notebook` },
  );
  assert.equal(answer.status, 200);
  const { checkout, ...rest } = await answer.json();
  assert.match(checkout, /^co_/);
  assert.deepEqual(Object.keys(rest), ["url"]);
  assert.equal((await site.payments.checkout.get(checkout)).amountTotal, 900);

  assert.equal(
    checkoutResponse(checkout, "/cancel").headers.get("location"),
    `${SHOP}/shop?size=a5`,
  );
  assert.equal(
    checkoutResponse(checkout, "/pay").headers.get("location"),
    `${SHOP}/shop?size=a5&checkout=${checkout}`,
  );
  const [last] = await site.payments.variants(["price_last"]);
  assert.equal(last.stock, 0);

  const returnsTo = async (body, headers) => {
    const made = await cartCheckout(
      { items: [{ variant: "price_pocket", quantity: 1 }], ...body },
      headers,
    ).json();
    return [
      checkoutResponse(made.checkout, "/pay").headers.get("location"),
      checkoutResponse(made.checkout, "/cancel").headers.get("location"),
    ];
  };
  const [paid, cancelled] = await returnsTo(
    { successUrl: "/thanks", cancelUrl: "/cart?from=checkout" },
    { referer: `${SHOP}/shop` },
  );
  assert.match(paid, /^http:\/\/localhost:1234\/thanks\?checkout=co_/);
  assert.equal(cancelled, `${SHOP}/cart?from=checkout`);
  for (const headers of [{}, { referer: "https://elsewhere.test/shop" }]) {
    const [home, back] = await returnsTo({}, headers);
    assert.match(home, /^http:\/\/localhost:1234\/\?checkout=co_/);
    assert.equal(back, `${SHOP}/`);
  }
  const [tunnelled] = await returnsTo(
    {},
    {
      "x-forwarded-host": "shop.tunnel.test",
      origin: "https://shop.tunnel.test",
      referer: "https://shop.tunnel.test/shop",
    },
  );
  assert.match(tunnelled, /^https:\/\/shop\.tunnel\.test\/shop\?checkout=co_/);
});

test("the cart's checkout sells only what the payment account sells", async (t) => {
  await paymentsSite(t, PAYMENTS);
  const refused = async (body, headers) => {
    const answer = cartCheckout(body, headers);
    return [answer.status, await answer.json()];
  };
  const pocket = [{ variant: "price_pocket", quantity: 1 }];
  assert.deepEqual(await refused("not json"), [
    400,
    { error: "A checkout is a JSON object with items" },
  ]);
  assert.deepEqual(await refused({}), [
    400,
    { error: "items must be a non-empty list" },
  ]);
  for (const item of [
    { name: "Tip", amount: 100, quantity: 1 },
    { variant: "", quantity: 1 },
    "price_pocket",
  ]) {
    assert.deepEqual(await refused({ items: [...pocket, item] }), [
      400,
      { error: "Every item needs the id of a variant the site sells" },
    ]);
  }
  assert.deepEqual(
    await refused({ items: [{ variant: "price_pocket", quantity: 0 }] }),
    [400, { error: "items[0].quantity must be a whole number of at least 1" }],
  );
  for (const successUrl of ["https://elsewhere.test/", "//elsewhere.test/", "thanks"]) {
    assert.deepEqual(await refused({ items: pocket, successUrl }), [
      400,
      { error: "successUrl must be a path on this site" },
    ]);
  }
  assert.deepEqual(await refused({ items: pocket, cancelUrl: 1 }), [
    400,
    { error: "cancelUrl must be a path on this site" },
  ]);
  assert.deepEqual(
    await refused({ items: pocket, shipping: { countries: ["USA"] } }),
    [
      400,
      { error: "shipping.countries must be a list of two-letter country codes" },
    ],
  );
  assert.equal(
    cartCheckout({ items: pocket, shipping: { countries: ["us"] } }).status,
    200,
  );
  assert.deepEqual(
    await refused({ items: [{ variant: "price_gone", quantity: 1 }] }),
    [
      422,
      {
        error:
          "price_gone is not a variant this site's payment account sells",
      },
    ],
  );
  assert.deepEqual(
    await refused({
      items: [...pocket, { variant: "price_last", quantity: 2 }],
    }),
    [409, { error: "out_of_stock", outOfStock: ["price_last"] }],
  );

  const get = cartResponse("/_archival/checkout", "GET", {}, null);
  assert.equal(get.status, 405);
  assert.equal(get.headers.get("allow"), "POST");
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

test("site.email keeps what a deploy would send, to show rather than deliver", async () => {
  const [, , , site] = await invokeCarrier(
    { carrier: (...args) => args, api: 2 },
    new URLSearchParams(),
    null,
    { objects: {}, siteUrl: "", uploads: [], database: "" },
    "contact",
  );
  const receipt = await site.email.send({
    to: "ana@example.com",
    subject: "Thanks, Ana",
    html: '<p>Hi <script>parent.stolen = 1</script>"Ana"</p>',
    text: "Hi Ana",
    replyTo: "owner@example.com",
  });
  assert.equal(receipt.from, "", "only a deploy knows the site's addresses");
  const email = sentEmail(receipt.id);
  assert.deepEqual(
    {
      carrier: email.carrier,
      from: email.from,
      to: email.to,
      subject: email.subject,
      replyTo: email.replyTo,
    },
    {
      carrier: "contact",
      from: null,
      to: ["ana@example.com"],
      subject: "Thanks, Ana",
      replyTo: "owner@example.com",
    },
  );
  const page = emailPage(email);
  assert.match(
    page,
    /<iframe class="body" sandbox="allow-popups allow-popups-to-escape-sandbox"/,
  );
  assert.ok(!page.includes("<script>"), "the body only ever reaches the sandbox");
  assert.match(
    page,
    /srcdoc="&lt;base target=&quot;_blank&quot;&gt;&lt;p&gt;Hi &lt;script&gt;/,
  );
  assert.match(page, /<summary>Plain text<\/summary><pre class="body">Hi Ana<\/pre>/);
});

test("site.email refuses what a deployed send refuses", async () => {
  const [, , , site] = await invokeCarrier(
    { carrier: (...args) => args, api: 2 },
    new URLSearchParams(),
    null,
    { objects: {}, siteUrl: "", uploads: [], database: "" },
  );
  for (const [message, reason] of [
    [{ to: "nope", subject: "x", text: "y" }, /"nope" is not an email address/],
    [{ to: "a@example.com", subject: "x" }, /text or html is required/],
    [{ to: "a@example.com", subject: "a\nb", text: "y" }, /line breaks/],
    [
      { to: Array(11).fill("a@example.com"), subject: "x", text: "y" },
      /at most 10 recipients/,
    ],
  ]) {
    await assert.rejects(site.email.send(message), (error) => {
      assert.match(error.message, /^Sending email failed: /);
      assert.match(error.message, reason);
      return true;
    });
  }
});
