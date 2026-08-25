# `archival` — build an Archival website with a coding agent

A Claude Code plugin that turns a short conversation into a working
[Archival](https://archival.dev) site: it interviews you, writes the site, and
publishes it to a shareable preview URL.

Aimed at someone who wants a website, not at someone who wants to learn a static
site generator. It asks about your business and your words, and makes the
technical decisions itself.

## Install

```
/plugin marketplace add archival-dev/archival
/plugin install archival@archival
```

Installing the plugin also connects the `archival` MCP server, which is what the
skill drives. Then just say what you want:

> Build me a site for my bakery in Joshua Tree.

## Publishing

Nothing is installed and nothing is built locally: the site is written straight
into the preview through the MCP tools, and Archival builds it on its side with
a pinned `archival` binary.

**A person has to approve the session in a browser.** The agent asks for a link
and a code, you open the link, clear a challenge, and approve — that is the only
gate, and it is why no credential has to be pasted into the session. Starting
from <https://archival.dev> does the same check up front and hands you a
ready-to-paste prompt with the session already approved.

Previews are public, expire if nobody claims them, and every publish is reviewed.

## What it builds

Static content sites: marketing pages, portfolios, brochures, landing pages,
event and menu pages.

## Contents

| | |
|---|---|
| `skills/new/SKILL.md` | the workflow |
| `skills/new/reference/authoring.md` | objects, fields, templates, layouts, partials |
| `skills/new/reference/publishing.md` | the same flow over plain HTTP, for when the MCP tools are not connected |
| `bin/install-archival.sh` | downloads a pinned `archival` release, checksum-verified |

The MCP server is declared in `.claude-plugin/plugin.json`, so installing the
plugin is the only step. `install-archival.sh` is now optional — it is there for
`archival run`'s live reload in a local session, not for building a preview.

`install-archival.sh` records no version. It takes one, falls back to this
repo's `Cargo.toml`, then to the latest release — so a release bump reaches it
with no edit, and `check-versions.sh` fails if that inference ever breaks.

## Using it without Claude Code

Nothing here is Claude-specific. The MCP server at
<https://api.archival.dev/mcp> works with any MCP client, and the same flow is a
documented HTTP API for anything that has neither:

- <https://archival.dev/agent/build-site.md>
- <https://archival.dev/agent/reference/authoring.md>
- <https://archival.dev/agent/reference/publishing.md>

Those are mirrored from this directory at build time, so they cannot drift.
A Claude Code **cloud** session should read them from
`raw.githubusercontent.com` instead — that host is on the default network
allowlist and `archival.dev` is not.

## License

AGPL-3.0-or-later, like the rest of this repository.
