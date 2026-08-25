---
name: new
description: Build a website with Archival from a short conversation, answer questions about how Archival works, and publish it to a shareable preview URL. Use when someone wants a new website, landing page, portfolio, or brochure site built with Archival, asks what Archival is or how it works, or arrives with an archival.dev preview session.
---

# Build an Archival site

Archival is a static site generator that keeps **content** (TOML object files)
separate from **presentation** (Liquid templates). You are going to interview
someone about the website they want, build it, show it to them, and publish it.

The person you are talking to is usually **not a developer**. Do not ask them
about TOML, Liquid, or directory layout. Ask about their business and their
words; make every technical decision yourself.

You need no local checkout, no working directory and no toolchain. The site is
written straight into the preview and built on Archival's side.

## 1. Get a session

The `archival_*` tools do all of this.

A request launched from archival.dev carries a **session** in the prompt — use
it as-is and skip to step 2. Otherwise:

1. `archival_start_session`, with a short `slug` for the site if you already
   know one. It returns a link and a code.
2. Show the person **both**, and ask them to check the code matches before
   approving. That check is what stops a link someone else sent them from being
   approved by mistake.
3. Poll `archival_await_approval` every couple of seconds until it stops saying
   `waiting`.

Do something useful while you wait — ask them about the site rather than
watching a spinner. The link expires in ten minutes and the session lasts two
hours. Never invent a session or a preview name.

If the `archival_*` tools are not there, say so plainly rather than improvising.
In a Claude conversation they arrive as a connector: ask the person to add
`https://api.archival.dev/mcp` as a custom connector under Settings →
Connectors, then start a new chat — a connector added partway through does not
appear in the conversation already running. Where you have a shell instead,
`reference/publishing.md` is the same flow over plain HTTP and needs nothing
added.

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

Answer from the documentation rather than from memory: `archival_reference` with
topic `authoring` covers the model, and topic `docs` is the full corpus. If you
do not know, say so and point them at <https://archival.dev/docs>, which they can
open even when you cannot.

## 3. Ask what goes on the site

Open with the question that matters most:

> **What kinds of things will you put on your website?**

You are listening for the things there will be _more than one of_ — menu items,
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

## 4. Write the site

```
archival.toml             site config (site_name, site_url)
archival_objects.toml     the content schema
objects/                  the content itself
pages/                    Liquid page templates
layout/                   shared page chrome
public/                   static files copied verbatim into the build
```

**Read `archival_reference` with topic `authoring` before writing any of
these.** It covers the object schema, field types, child fields, page templates,
partials and layouts, and is the difference between a site that builds and one
that does not. Do not write Archival syntax from memory — the format is specific
and it has changed.

Write the files with `archival_write_files`, which takes a batch. Read one back
with `archival_read_file`, remove one with `archival_delete_files`.

None of this belongs on the machine you are running on. If you do write files
there — a copy to run the CLI against, notes to yourself — make a new directory
for them rather than working wherever the session happened to open.

Images and other media do not go in the source. `archival_upload_media` puts
them on the CDN and hands back the `sha`, `filename`, `mime` and `display_type`
to write into the object file; archival builds the URL from those.

## 5. Design it properly

The generator is not the interesting part; the site is. Aim for something they
would be glad to send to a customer.

- Write real CSS in `public/`. Pick a deliberate type scale, a restrained
  palette, and generous spacing.
- Make it responsive and keep it accessible: sensible heading order, alt text on
  every image, and text that passes contrast against its background.
- Prefer few, well-made sections over many thin ones.
- Set the page `<title>` and meta description for every page.

## 6. Publish

`archival_publish` builds the site with Archival and puts it live. It is the
check that matters: a template or schema error fails here, and what comes back is
archival's own diagnostic. Fix what it names and publish again. Never claim a
site works without a successful publish.

Say before the first publish that the URL is public and that every publish is
reviewed. Publish nothing they did not ask for.

Publish early and iterate on the live URL — that is the fastest feedback loop
there is. When they are happy, give them the URL and tell them they can keep the
site by opening it and claiming it.

### Optionally, a local preview

In a local session, if they want live reload while you work, install the CLI and
run a copy of the site from a directory of your own:

```bash
bash "${CLAUDE_PLUGIN_ROOT:-.}/bin/install-archival.sh"
archival run <site dir>    # rebuilds on change, serves on http://localhost:1024
```

This is a convenience, not a step. Publishing is what shows them the real thing,
and it is the only thing that works in a cloud session.

## Scope

Static content sites: marketing pages, portfolios, brochures, landing pages,
event and menu pages.

Carriers — Archival's serverless functions — are not available here and cannot
be previewed; a preview whose source contains `carriers/` is refused. If someone
needs forms that submit, logins, or a database, say that plainly and point them
at <https://archival.dev/docs/carriers.html>.
