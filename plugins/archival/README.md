# `archival` — build an Archival website with a coding agent

A Claude Code plugin that turns a short conversation into a working
[Archival](https://archival.dev) site. It interviews you, writes the site, and
either publishes it to a shareable preview URL or, in a folder, builds it right
there with the archival CLI.

Aimed at someone who wants a website, not at someone who wants to learn a static
site generator. It asks about your business and your words, and makes the
technical decisions itself.

## Install

```
/plugin marketplace add archival-dev/archival
/plugin install archival@archival
```

Installing the plugin also connects the `archival` MCP server. Then just say
what you want:

> Build me a site for my bakery in Joshua Tree.

## Two ways to use it

**In a chat** (claude.ai with the Archival connector, or any MCP client) nothing
is installed and nothing is built locally: the site is written straight into a
preview through the MCP tools, and Archival builds it on its side with a pinned
`archival` binary. A person approves the session in a browser — the agent asks
for a link and a code, you open the link, clear a challenge, and approve — and
that is the only gate, which is why no credential has to be pasted into the
session. Starting from <https://archival.dev> does the same check up front and
hands you a ready-to-paste prompt with the session already approved.

**In a folder** (Claude Code) the skill notices it has a shell and works there.
`bin/site-info.sh` tells it whether the folder already holds an Archival site;
`bin/install-archival.sh` puts the CLI in `~/.archival/bin`; every change is
checked with `archival build` before you see it, and `archival run` serves the
site locally with live reload. A brand-new site is published to a preview once,
because that link is how you claim it. Once you have claimed it and run
`archival login`, the skill connects the folder to your site's git remote at
`git.archival.dev/<your domain>`, and from then on `git push` deploys and
`archival upload` carries images and files to the CDN. An existing site in the
folder is handled by `/archival:site`; a session that opens in one is told so.

Previews are public, expire if nobody claims them, and every publish is reviewed.

## What it builds

Static content sites: marketing pages, portfolios, brochures, landing pages,
event and menu pages — with the content in objects the owner can edit, an
`archival_editor.toml`, and the pages machines read (`llms.txt`, a feed, a
sitemap).

## Contents

| | |
|---|---|
| `skills/new/SKILL.md` | the workflow, with the chat / folder decision up front |
| `skills/site/SKILL.md` | working on the Archival site in the current folder |
| `skills/new/reference/authoring.md` | objects, fields, templates, layouts, partials |
| `skills/new/reference/conventions.md` | what keeps a site editable: objects, `archival_editor.toml`, robot-facing pages, metadata, a checklist |
| `skills/new/reference/local.md` | the folder workflow: install, build, media, go live, link the folder, deploy |
| `skills/new/reference/publishing.md` | the preview flow over plain HTTP, for a shell without the MCP tools |
| `bin/site-info.sh` | describes a directory as the skills need to see it |
| `bin/install-archival.sh` | downloads a pinned `archival` release into the directory you name, checksum-verified |
| `hooks/` | a SessionStart hook that announces an Archival site in the project directory |
| `test.sh` | syntax and fixture checks for the scripts; run by `pre-commit.sh` and CI |

The MCP server is declared in `.claude-plugin/plugin.json`, so installing the
plugin is the only step.

`install-archival.sh` records no version. It takes one, falls back to this
repo's `Cargo.toml`, then to the latest release — so a release bump reaches it
with no edit, and `check-versions.sh` fails if that inference ever breaks.

## Using it without Claude Code

Nothing here is Claude-specific. The MCP server at
<https://api.archival.dev/mcp> works with any MCP client, and the same flow is a
documented HTTP API for anything that has neither:

- <https://archival.dev/agent/build-site.md>
- <https://archival.dev/agent/edit-site.md>
- <https://archival.dev/agent/reference/authoring.md>
- <https://archival.dev/agent/reference/conventions.md>
- <https://archival.dev/agent/reference/local.md>
- <https://archival.dev/agent/reference/publishing.md>

Those are mirrored from this directory at build time, so they cannot drift.
A Claude Code **cloud** session should read them from
`raw.githubusercontent.com` instead — that host is on the default network
allowlist and `archival.dev` is not.

## License

AGPL-3.0-or-later, like the rest of this repository.
