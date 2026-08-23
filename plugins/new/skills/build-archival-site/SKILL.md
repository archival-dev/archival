---
name: build-archival-site
description: Build a website with Archival from a short conversation, preview it locally with the archival CLI, and publish it to a shareable preview URL. Use when someone wants a new website, landing page, portfolio, or brochure site built with Archival, or arrives with an archival.dev publish token.
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

If any are missing you can still build the site — just skip step 6 and tell them
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

## 2. Interview — briefly

Ask **three or four questions in one message**, then build. People came here for
a website, not a form.

Cover: what the site is for and who it's for; the name and one-line pitch; the
sections they want (or propose some and let them correct you); and anything they
already have — text, images, colors, an existing site to draw from.

Two rules that matter more than they look:

- **Never invent facts about a real business.** No fake testimonials, no invented
  addresses, phone numbers, prices, hours, or credentials. Use their words, or
  clearly-marked placeholder text they can replace.
- **One round of questions.** If they give you little, make strong choices and
  show them something — reacting to a real page beats answering more questions.

## 3. Scaffold

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

## 4. Design it properly

The generator is not the interesting part; the site is. Aim for something they
would be glad to send to a customer.

- Write real CSS in `public/`. Pick a deliberate type scale, a restrained
  palette, and generous spacing. Avoid the default-Bootstrap look.
- Make it responsive and keep it accessible: sensible heading order, alt text on
  every image, and text that passes contrast against its background.
- Prefer few, well-made sections over many thin ones.
- Set the page `<title>` and meta description for every page.

## 5. Build, check, and show them

```bash
archival build .          # validates and renders into dist/
archival run .            # rebuilds on change, serves on http://localhost:1024
```

`archival build` is the check that matters — a template or schema error fails
here, and you should never publish or claim success without a clean build.

**`archival run` only helps in a local session.** In a Claude Code *cloud*
session the port is not reachable from their browser, so skip it there and let
publishing (step 6) be how they see the site.

Show them the real thing early and iterate on their reactions.

## 6. Publish

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
