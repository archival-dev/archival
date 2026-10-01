---
name: site
description: Work on the Archival site in the current folder - add pages or kinds of content, add or edit content, fix a failing archival build, upload images and files, connect the folder to a hosted archival.dev site, and deploy. Use when the folder contains archival_objects.toml (or a legacy objects.toml), when someone asks to change, fix, publish or deploy their Archival site, or when a session-start note says this folder is an Archival site.
---

# Work on the Archival site in this folder

This folder is the site. You have a shell, the `archival` CLI builds here, and
the hosted platform is reached through git and `archival upload`. The person
may be a developer or may not; either way, every change keeps the site one they
can go on editing in the Archival editor.

The shared references live beside the `new` skill:
`${CLAUDE_PLUGIN_ROOT}/skills/new/reference/local.md` (the commands),
`conventions.md` (what a good site looks like, and the checklist) and
`authoring.md` (the TOML and Liquid archival actually accepts). Read
`authoring.md` before touching a schema or a template; the syntax is specific.

## 1. Look first

```bash
bash "${CLAUDE_PLUGIN_ROOT}/bin/site-info.sh"
```

- `site=no`: this is not an Archival site. Hand over to the `archival:new`
  skill rather than inventing one here.
- `site=legacy`: archival still reads `manifest.toml` and `objects.toml`. Do
  not rename them unless asked.
- `archival=missing`: install it, never into the repo:
  ```bash
  bash "${CLAUDE_PLUGIN_ROOT}/bin/install-archival.sh" "" "$HOME/.archival/bin"
  ```
  then `export PATH="$HOME/.archival/bin:$PATH"` in each command that needs it.
- `remote=` and `logged_in=` decide how it deploys and whether media can be
  uploaded (steps 5 and 6).

## 2. Baseline

```bash
export PATH="$HOME/.archival/bin:$PATH"
archival validate --json . && archival build .
```

A site that does not build before you touch it is reported to the person, not
silently repaired. Then read `archival_objects.toml`, `objects/`, `pages/`,
`layout/` and `archival_editor.toml` so the change fits what is there.

## 3. Make the change the Archival way

- **Content goes in objects.** A new paragraph, price, name or date is a field
  on an object, never text in a `.liquid` file (conventions §1).
- **A new kind of thing** is: a type in `archival_objects.toml` → its files
  under `objects/<type>/` → `pages/<type>.liquid` with `template = "<type>"` if
  visitors click through to one → a `[[<type>.views]]` entry in
  `archival_editor.toml`, and a shortcut if they will add to it often → a line
  in `llms.txt` and `sitemap.xml` → `archival build`.
- **Changing a field's type**: the definition, then every object file, then the
  templates that read it. `archival validate --json .` catches the objects.
- **Design** lives in `layout/`, `pages/` and `public/`.
- Never edit `dist/`, `.github/workflows/archival.yml`, or the manifest's
  `upload_prefix`, `uploads_url`, `site_url` and `metadata`. The platform owns
  those.
- Build after each file. The error names the file you just wrote.

## 4. Verify

```bash
rm -rf dist && archival build . && archival format .
```

then the checklist in conventions §11. `archival run . -p 1024` serves the site
at `http://localhost:1024` with live reload when the person wants to look. Say
that address is only on this computer.

## 5. Media

```bash
archival upload <type>/<filename> <field> <file>    # a list item
archival upload <root> <field> <file>               # a root object
```

It hashes and uploads the file and writes the `sha`/`filename`/`mime`/
`display_type` block into the object itself. It needs `logged_in=yes` (ask them
to run `archival login`, or run it in the background and relay the URL it
prints) and a remote it can read (`remote=archival` or `remote=github`), or
`-r domain/<domain>`. Never `git add` an image, a video or a download; a push
over 100 MB fails outright.

## 6. Commit and deploy

- `remote=archival`: `git push origin main` deploys.
- `remote=github` with `workflow=yes`: `git push` deploys through the site's
  Actions workflow.
- `remote=none`: the folder is not connected to a hosted site yet. Follow
  `local.md` G to list the person's sites and mint a remote, or F if the site
  has never been published anywhere.

Deploy progress: `GET https://api.archival.dev/sites/github/<owner>/<name>`
with the login token (`local.md` F.6 shows how to read it without printing it),
under `setupInfo.deploy`.

Commit messages describe the change to the site, in the person's terms.
