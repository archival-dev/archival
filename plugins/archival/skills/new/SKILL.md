---
name: new
description: Build a website with Archival from a short conversation, answer questions about how Archival works, preview the site locally with the archival CLI, and publish it to a shareable preview URL. Use when someone wants a new website, landing page, portfolio, or brochure site built with Archival, asks what Archival is or how it works, or arrives with an archival.dev publish token.
---

# Build an Archival site

Archival is a static site generator that keeps **content** (TOML object files)
separate from **presentation** (Liquid templates). You are going to interview
someone about the website they want, build it, show it to them, and publish it.

The person you are talking to is usually **not a developer**. Do not ask them
about TOML, Liquid, or directory layout. Ask about their business and their
words; make every technical decision yourself.

## What arrives with the request

A request launched from archival.dev carries three values in the prompt:

| Value | Use |
|---|---|
| **Publish token** (`archival-preview-pk-…`) | Bearer token for publishing. Scoped to one preview, expires in ~2 hours. |
| **Preview name** | The site's subdomain. Already reserved for this person. |
| **Archival version** | The exact CLI version to install. Matches what the server builds with. |

If they are missing — someone invoked this directly rather than arriving from
archival.dev — get your own with the pairing flow below. Never invent a token or
a preview name.

### Pairing, when no token arrived

```bash
curl -sS -X POST https://api.archival.dev/previews/self-serve/pair/start
```

Returns `{ "code": "...", "verifyUrl": "https://archival.dev/link?c=...", … }`.
Show the person **both** the URL and the code, and ask them to open it and check
the code matches before approving — that check is what stops a link someone else
sent them from being approved by mistake.

Then poll, about every two seconds, until it stops returning `204`:

```bash
curl -sS -o /dev/null -w '%{http_code}' -X POST -d "$CODE" \
  https://api.archival.dev/previews/self-serve/pair/poll
```

- `204` — not approved yet, keep waiting
- `200` — the body is the same payload `start` returns: token, preview name,
  uploads URL and prefix, archival version. Use it exactly as if it had arrived
  in the prompt.
- `404` — the code expired (ten minutes) or was already collected. Start again.

Do something useful while you wait — ask them about the site rather than
watching a spinner. The code is single-use and the grant lasts two hours.

### Check what you can reach, first

Sessions differ in what they can talk to, and both of these change what you
should do. Check once, before anything else:

```bash
curl -sS -o /dev/null -w 'publish:%{http_code}\n' -m 10 \
  -H "Authorization: Bearer $TOKEN" \
  https://api.archival.dev/previews/self-serve/status
curl -sS -o /dev/null -w 'docs:%{http_code}\n' -m 10 \
  https://archival.dev/docs/llms.txt
```

**Publishing** — `200` on the first. Build, publish early, and iterate on the
live URL; that is the fastest feedback loop there is, so prefer it whenever it
is available.

**Docs** — `200` on the second. `/docs/llms.txt` is the whole reference in one
fetch, so prefer it. Without it, use the files next to this one on
`raw.githubusercontent.com`, and read the archival source there too if a
question goes deeper — it is a public repository. Everything except the hosted
editor is documented or readable somewhere you can get to.

A `403` carrying `x-deny-reason: host_not_allowed` means the session's network
policy does not include that host. On a Claude Code cloud session that is the
default, and the policy is fixed when the session starts — nothing in here can
change it. A `401` on the first means the token is wrong or its two hours have
passed, which is a different problem and worth saying so.

### If you cannot publish

Say so before you start building, then offer both and let them pick:

1. **Start again locally**, where all of this works: install Claude Code, open a
   terminal, and paste the same prompt. Shortest path, and the one to recommend.
2. **Build here anyway.** You can still author the whole site and validate it
   with `archival build`. Then either hand them the upload commands from
   `reference/publishing.md` to run locally against the same token, or have them
   restart in a cloud environment allowing `api.archival.dev` (added at
   <https://claude.ai/code>; an existing session cannot be moved into one).

Either way keep working — do not stop and wait.

## 1. Install the CLI

```bash
bash "${CLAUDE_PLUGIN_ROOT:-.}/bin/install-archival.sh" <archival version>
```

Not running as a plugin? Fetch the script from
`https://raw.githubusercontent.com/archival-dev/archival/main/plugins/archival/bin/install-archival.sh`.
It downloads the release from GitHub, verifies the published SHA-256, and prints
the binary path. Use that path everywhere below as `archival`.

**Pass the version from the request.** Given no argument the script resolves one
itself — the repo's `Cargo.toml`, then the latest release — which is right for
casual use but not here. The version in the request is the one this person's
site will be edited and rebuilt with if they keep it, so building against a
different one means what they approved is not what they get.

## 2. Say what Archival is, in two sentences

Most people arriving here have never heard of it. Before asking anything, tell
them plainly what they are getting, in their words rather than ours:

> Archival keeps the **stuff on your site** separate from the **way it looks**.
> You get a place to add and edit your content whenever you like, and changing
> it never risks breaking the design.

Then make it clear you can go deeper: **you can answer questions about Archival
at any point** — how it works, what it costs, how to edit the site later, how to
connect a domain, whether it can do some particular thing. Offer that once, here,
and answer whenever they ask.

Answer from the documentation rather than from memory: `reference/authoring.md`
covers the model, and `/docs/llms.txt` is the full corpus when you can reach it.
If you do not know, say so and point them at <https://archival.dev/docs>, which
they can open even when you cannot.

## 3. Ask what goes on the site

Open with the question that matters most:

> **What kinds of things will you put on your website?**

You are listening for the things there will be *more than one of* — menu items,
events, projects, posts, staff, products, testimonials, locations. Each of those
becomes a type of entry they can add to forever without touching the design.
Things there is only one of — an about page, contact details, the homepage
headline — are just as valid, they are simply single entries.

Say that back to them in their own nouns, so the division lands early:

> So we will set you up with **events** and **menu items** you can add to
> whenever you like, plus your **about** page.

Getting this right first is what makes the site theirs afterwards. A page of
hand-written HTML is a page they must ask someone to change; a list of events is
a list they can add to. When in doubt, make it an entry.

Then, in the **same message**, ask two or three supporting questions: the name
and one-line pitch, anything they already have (text, photos, colours, an
existing site), and roughly the feel they want. Then build. People came here for
a website, not a form.

Three rules that matter more than they look:

- **Never invent facts about a real business.** No fake testimonials, no invented
  addresses, phone numbers, prices, hours, or credentials. Use their words, or
  clearly-marked placeholder text they can replace.
- **One round of questions.** If they give you little, make strong choices and
  show them something — reacting to a real page beats answering more questions.
- **Start simple.** Two or three entry types and a clean single page beats an
  elaborate structure they did not ask for. They can see how it fits together,
  and adding a type later costs nothing.

## 4. Scaffold

```
archival.toml             site config (site_name, site_url)
archival_objects.toml     the content schema
objects/                  the content itself
pages/                    Liquid page templates
layout/                   shared page chrome
public/                   static files copied verbatim into the build
```

Read `reference/authoring.md` before writing any of these. It covers the object
schema, field types, child fields, page templates, partials and layouts, and is
the difference between a site that builds and one that does not.

Running as a plugin, they are alongside this file. Otherwise fetch them here —
`raw.githubusercontent.com` is reachable even when `archival.dev` is not:

- `https://raw.githubusercontent.com/archival-dev/archival/main/plugins/archival/skills/new/reference/authoring.md`
- `https://raw.githubusercontent.com/archival-dev/archival/main/plugins/archival/skills/new/reference/publishing.md`

## 5. Design it properly

The generator is not the interesting part; the site is. Aim for something they
would be glad to send to a customer.

- Write real CSS in `public/`. Pick a deliberate type scale, a restrained
  palette, and generous spacing.
- Make it responsive and keep it accessible: sensible heading order, alt text on
  every image, and text that passes contrast against its background.
- Prefer few, well-made sections over many thin ones.
- Set the page `<title>` and meta description for every page.

## 6. Build and show them

```bash
archival build .          # validates and renders into dist/
archival run .            # rebuilds on change, serves on http://localhost:1024
```

`archival build` is the check that matters: a template or schema error fails
here. Never claim a site works without a clean build.

`archival run` is worth it in a local session and useless in a cloud one, where
the port is not reachable from their browser. There, publishing is how they see
it.

Show them something real early and iterate on what they say about it.

## 7. Publish

`reference/publishing.md` has the calls: upload the site source and the built
`dist/`, then publish. It returns the live URL. Media goes through the upload
endpoint first — see that file.

Say before the first publish that the URL is public and that every publish is
reviewed. Publish nothing they did not ask for.

Re-publishing after edits is the normal loop. When they are happy, give them the
URL and tell them they can keep the site by opening it and claiming it.

## Scope

Static content sites: marketing pages, portfolios, brochures, landing pages,
event and menu pages.

Carriers — Archival's serverless functions — are not available here and cannot
be previewed. If someone needs forms that submit, logins, or a database, say
that plainly and point them at <https://archival.dev/docs/carriers.html>.
