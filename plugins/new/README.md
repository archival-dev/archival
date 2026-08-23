# `new` — build an Archival website with a coding agent

A Claude Code plugin that turns a short conversation into a working
[Archival](https://archival.dev) site: it interviews you, writes the site,
builds it with the real `archival` CLI, and publishes it to a shareable preview
URL.

Aimed at someone who wants a website, not at someone who wants to learn a static
site generator. It asks about your business and your words, and makes the
technical decisions itself.

## Install

```
/plugin marketplace add jesseditson/archival
/plugin install new@archival
```

Then just say what you want:

> Build me a site for my bakery in Joshua Tree.

## Publishing

Building works on its own. **Publishing needs a token**, which you get from
<https://archival.dev/build-with-claude> — that page also hands you a
ready-to-paste prompt carrying the token, so starting there is the shortest
path.

Without one, the agent still builds and previews the site locally; it just has
nowhere to publish it.

Previews are public, expire if nobody claims them, and every publish is reviewed.

## What it builds

Static content sites: marketing pages, portfolios, brochures, landing pages,
event and menu pages.

## Contents

| | |
|---|---|
| `skills/build-archival-site/SKILL.md` | the workflow |
| `skills/build-archival-site/reference/authoring.md` | objects, fields, templates, layouts, partials |
| `skills/build-archival-site/reference/publishing.md` | the publish HTTP contract |
| `bin/install-archival.sh` | downloads a pinned `archival` release, checksum-verified |

`install-archival.sh` records no version. It takes one, falls back to this
repo's `Cargo.toml`, then to the latest release — so a release bump reaches it
with no edit, and `check-versions.sh` fails if that inference ever breaks.

## Using it without Claude Code

Nothing here is Claude-specific. The contract is a skill document, the public
`archival` binary, and a documented HTTP API, so any coding agent can follow it:

- <https://archival.dev/agent/build-site.md>
- <https://archival.dev/agent/reference/authoring.md>
- <https://archival.dev/agent/reference/publishing.md>

Those are mirrored from this directory at build time, so they cannot drift.
A Claude Code **cloud** session should read them from
`raw.githubusercontent.com` instead — that host is on the default network
allowlist and `archival.dev` is not.

## License

AGPL-3.0-or-later, like the rest of this repository.
