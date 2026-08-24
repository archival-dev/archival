---
name: build-archival-site
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

If any are missing you can still build the site — just skip step 7 and tell them
to get a link from <https://editor.archival.dev/build-with-claude>. Never invent
a token or a preview name.

## 1. Install the CLI

```bash
bash "${CLAUDE_PLUGIN_ROOT:-.}/bin/install-archival.sh" <archival version>
```

Not running as a plugin? Fetch the script from
`https://raw.githubusercontent.com/jesseditson/archival/main/plugins/new/bin/install-archival.sh`.
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

You have the documentation for this. `reference/authoring.md` covers the model,
and <https://archival.dev/docs/llms.txt> is the full corpus if they ask something
it does not cover. Answer from those rather than from memory — and if you do not
know, say so and point at <https://archival.dev/docs>.

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

Read `reference/authoring.md` before writing any of these — it covers the object
schema, field types, child fields, page templates, partials, and layouts, and it
is the difference between a site that builds and one that does not.

## 5. Design it properly

The generator is not the interesting part; the site is. Aim for something they
would be glad to send to a customer.

- Write real CSS in `public/`. Pick a deliberate type scale, a restrained
  palette, and generous spacing. Avoid the default-Bootstrap look.
- Make it responsive and keep it accessible: sensible heading order, alt text on
  every image, and text that passes contrast against its background.
- Prefer few, well-made sections over many thin ones.
- Set the page `<title>` and meta description for every page.

## 6. Build, check, and show them

```bash
archival build .          # validates and renders into dist/
archival run .            # rebuilds on change, serves on http://localhost:1024
```

`archival build` is the check that matters — a template or schema error fails
here, and you should never publish or claim success without a clean build.

**`archival run` only helps in a local session.** In a Claude Code *cloud*
session the port is not reachable from their browser, so skip it there and let
publishing (step 7) be how they see the site.

Show them the real thing early and iterate on their reactions.

## 7. Publish

Read `reference/publishing.md` for the exact HTTP calls. In short: upload the
site source and the built `dist/`, then publish. It returns the live URL.

Publishing is public — anything you upload is on the internet at that URL, and
each publish is reported to Archival for abuse review. Say so before the first
publish, and never publish content the person did not ask for.

Re-publishing after edits is the normal loop. When they are happy, give them the
URL and tell them they can keep the site by opening it and choosing to claim it.

## Scope

Build static content sites: marketing pages, portfolios, brochures, landing
pages, event and menu pages.

**Do not use carriers** (Archival's serverless functions). They are compiled out
of published CLI binaries, so they cannot run or be previewed here, and a
self-serve preview does not deploy them. If someone needs forms that submit,
logins, or a database, say plainly that this flow builds static sites and point
them at <https://archival.dev/docs/carriers.html>.
