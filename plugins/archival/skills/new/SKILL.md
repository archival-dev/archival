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

**A live URL is the deliverable, and it comes early.** The whole shape is: get
approved, ask one round of questions, publish something real within a few
minutes, then improve it together while they watch. A session gets 20 publishes
— they are there to be spent. Someone who has asked twice for a website and has
no link yet has been failed, however good the thing you are still writing is.

## 1. Get a session

**Check the `archival_*` tools are there before anything else.** If they are
not, say so plainly and stop rather than improvising a way around it. In a Claude
conversation the tools arrive as a connector, so give the person the link and
the URL together:

> Add the Archival connector at
> <https://claude.ai/customize/connectors?modal=add-custom-connector&connectorName=Archival&connectorUrl=https%3A%2F%2Fapi.archival.dev%2Fmcp>,
> then start a new chat.

That is an install link: it opens the dialog with the name and URL already in
it, so all they do is confirm. Give it whole — a shortened or retyped version
loses the prefill. If they would rather add it by hand, the server URL is
`https://api.archival.dev/mcp`. A connector added partway through does not
appear in the conversation already running, which is why the new chat is not
optional.

`reference/publishing.md` is the same flow over plain HTTP, but it only helps
where the shell you are in can actually reach `api.archival.dev`. A local
terminal can. A claude.ai sandbox and a Claude Code cloud session cannot — both
answer `403` with `x-deny-reason: host_not_allowed`, and nothing you can do from
inside changes that. If curl comes back with that, the connector is the only way
forward: say so and stop, rather than writing a site nobody can publish.

With the tools in hand, they do the rest of this on their own. A request
launched from archival.dev carries a **session** in the prompt — use it as-is
and skip to step 2. Otherwise:

1. `archival_start_session`, with a short `slug` for the site if you already
   know one. It returns a link and a code.
2. Send **one** message: the link, the code, ask them to check the code matches
   on the page before approving, and — in the same message — the questions from
   step 3. That check is what stops a link someone else sent them from being
   approved by mistake, and folding the questions in means the wait costs
   nothing.
3. Then stop and let them reply. Call `archival_await_approval` when they come
   back, or once or twice if you have something to say meanwhile.

The person is the blocker here, and nothing you can do removes them. Do not sit
in a polling loop, and do not push the wait into a background task and carry on
building: the link expires in ten minutes, and a link that lapses unwatched
while you write a site against a session that was never approved costs a whole
round trip and leaves you with nothing to show. If it has expired, mint another
and say so in one line. Never invent a session or a preview name.

The session lasts two hours once approved.

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

If you had to mint an approval link in step 1, all of this goes in that same
message. There is no reason to spend a round trip on the link alone.

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
- **A vague answer is not a reason to ask again.** Build something, publish it,
  and let them correct a real page.

## 4. Write the smallest site that renders

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
that does not. Do not write Archival syntax from memory — the format is specific,
it has changed, and it is not the Liquid any other generator uses. Its "Gotchas"
section is the part that actually costs builds: accessor names are inflected, so
a root object type called `settings` is `setting` in a template; `site_name` is
not a variable; `layout/` is singular; `include` and `render` are not
interchangeable.

Write the files with `archival_write_files`, which takes a batch. Read one back
with `archival_read_file`, remove one with `archival_delete_files`.

Aim this first pass at a page that renders, not at a finished site: the object
types, real content in them, one page that shows it, and a stylesheet you are
not done with. Then publish. Everything after that — more pages, media, the
design — goes faster against something they can already see.

None of this belongs on the machine you are running on. If you do write files
there — a copy to run the CLI against, notes to yourself — make a new directory
for them rather than working wherever the session happened to open.

Images and other media do not go in the source. `archival_upload_media` puts
them on the CDN and hands back the `sha`, `filename`, `mime` and `display_type`
to write into the object file; archival builds the URL from those.

## 5. Publish, and give them the link

`archival_publish` builds the site with Archival and puts it live. It is also the
only check there is — nothing validates a template until this runs — so call it
as soon as one page renders rather than at the end. A schema or template error
fails here and comes back as archival's own diagnostic. Fix what it names and
publish again. Never claim a site works without a successful publish.

Say before the first publish that the site is public and that every publish is
reviewed. Publish nothing they did not ask for.

Then **give them the `url` it returned, on its own line.** Not buried in a
summary of what you built — the link is the thing they have been waiting for,
and a message about the site that does not contain it has not delivered
anything. Share `url` and only `url`: the `siteUrl` beside it is the raw origin,
and it strands whoever opens it with no way to keep the site.

Republishing after edits is the loop, not an exception. Twenty publishes per
session is enough to work in front of them: change something, publish, ask what
they think of it.

When they are happy, tell them they can keep the site by opening that link and
claiming it.

## 6. Then make it good

Now that they can see it, make it something they would be glad to send to a
customer. Work in passes, and publish each one.

- Write real CSS in `public/`. Pick a deliberate type scale, a restrained
  palette, and generous spacing.
- Make it responsive and keep it accessible: sensible heading order, alt text on
  every image, and text that passes contrast against its background.
- Prefer few, well-made sections over many thin ones.
- Set the page `<title>` and meta description for every page.

### Optionally, a local preview

In a local session, if they want live reload while you work, install the CLI and
run a copy of the site from a directory of your own:

```bash
bash "${CLAUDE_PLUGIN_ROOT:-.}/bin/install-archival.sh"
archival run <site dir>    # rebuilds on change, serves on http://localhost:1024
```

This is a convenience, not a step, and it needs a shell — in a chat there is
none. Publishing is what shows them the real thing, and it is what they can
actually open.

## Scope

Static content sites: marketing pages, portfolios, brochures, landing pages,
event and menu pages.

Carriers — Archival's serverless functions — are not available here and cannot
be previewed; a preview whose source contains `carriers/` is refused. If someone
needs forms that submit, logins, or a database, say that plainly and point them
at <https://archival.dev/docs/carriers.html>.
