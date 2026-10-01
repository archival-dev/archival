# Code mode: an Archival site in a folder, with a shell

This is the workflow for Claude Code, or any agent with a shell, running in a
directory. If you have no shell — a claude.ai chat, the Archival connector, an
MCP client — none of this applies: `SKILL.md` steps 1–6 are your whole path.

Here the folder is the site, the `archival` CLI is the build, and the hosted
preview is used once, to show a brand-new site to the person and hand them the
link that claims it. After the claim, the folder's git remote is the site.

Keep `reference/conventions.md` open while you write. It is what makes the site
one the person can keep editing in the Archival editor after you are gone.

Three constraints shape every command below:

- **Shell state does not persist between tool calls.** Export `PATH` in each
  command that needs it, and keep tokens in files under `$HOME/.archival/`
  (mode 0600), never in variables you expect to find again.
- **Never print a credential.** Read a token with shell substitution inside the
  command that uses it. Neither the `~/.archivalrc` token nor a preview token
  may appear in your output.
- **Never install into the person's repository.** The CLI lives in
  `$HOME/.archival/bin`.

## A. Install or locate archival

`site-info.sh` reports `archival=<path> <version>` or `archival=missing`. Prefer
a binary already on `PATH`. Otherwise:

```bash
bash "${CLAUDE_PLUGIN_ROOT}/bin/install-archival.sh" "" "$HOME/.archival/bin"
export PATH="$HOME/.archival/bin:$PATH"; archival --version
```

The script downloads a checksum-verified release and prints the binary's path;
the empty first argument lets it pick the version. With no plugin root (you
reached these files by URL), fetch the script from
`https://raw.githubusercontent.com/archival-dev/archival/main/plugins/archival/bin/install-archival.sh`
and run it the same way.

## B. Detect a site

```bash
bash "${CLAUDE_PLUGIN_ROOT}/bin/site-info.sh"
```

| Key | Meaning |
|---|---|
| `site=yes` | `archival_objects.toml` is present: this is an Archival site. |
| `site=legacy` | `objects.toml` (and usually `manifest.toml`): an older layout archival still reads. Do not rename the files unless asked. |
| `site=no` | Not a site. With `empty_dir=yes` build here; with `empty_dir=no` ask before adding files, and offer a subfolder named for the site. |
| `remote=archival` | `origin` is `git.archival.dev/<domain>`: a push deploys. |
| `remote=github` | A GitHub remote. With `workflow=yes` a push deploys, and `archival upload` already knows the site. |
| `upload_prefix=set` | The manifest came from a claim or a preview pairing (see E). |
| `archival=…`, `logged_in=…` | Whether the CLI is installed and whether `archival login` has been run on this machine. |

The object-definition file is the whole test: `archival objects .` fails
without it. `[site_path]` is the current directory and archival never looks in
parent directories, so run every command from the site root.

## C. Create a new site

Each step ends with `archival build .` passing. Build after every file rather
than at the end: a schema or template error names the file you just wrote.

1. **Repo and ignores.** `git init -b main` if `git=no`. `.gitignore`:
   ```
   dist/
   .archival-bin/
   node_modules/
   .DS_Store
   ```
2. **`archival.toml`** with `site_name = "…"` and nothing else. Do not set
   `site_url`, `upload_prefix` or `uploads_url`: the platform writes them when
   the site is claimed (F), and a preview pairing sets the upload keys for the
   meantime (E).
3. **`archival_objects.toml`**: the `[site]` identity root (`name`,
   `description`, `og_image = "image"`, whatever the layout needs) and one list
   type per kind of thing there is more than one of, with `template = "<type>"`
   on anything a visitor clicks through to. Read `authoring.md` first; the
   format is specific. `archival validate --json .` checks this file and the
   objects without rendering.
4. **Content** in `objects/site.toml` and `objects/<type>/<slug>.toml`, each
   list item carrying `order`. Their words, or clearly marked placeholders.
   Never invented facts.
5. **`layout/theme.liquid`** and `public/style.css`. The `<head>` carries
   everything in conventions §6 from the first build.
6. **Pages**: `pages/index.liquid`, `pages/<type>.liquid` for each templated
   type, `pages/404.liquid`.
7. **Computer-facing pages**: `pages/llms.txt.liquid` always;
   `pages/sitemap.xml.liquid` and `pages/robots.txt.liquid`;
   `pages/feed.xml.liquid` when a dated list exists, linked from the layout.
8. **`archival_editor.toml`**: a `[[<type>.views]]` for every list type and a
   `[[shortcuts]]` for the one or two lists they will add to most.
9. **Finish**: `archival format . && rm -rf dist && archival build .`, the
   checklist in conventions §11, then `git add -A && git commit`.

A `Session:` in the prompt, or a request to see it live, does not change this
order. Build locally first; publishing is step F.

## D. The local loop

```bash
export PATH="$HOME/.archival/bin:$PATH"
archival build .                  # the only template check; writes dist/
archival validate --json .        # manifest, definitions, object files
archival format .                 # canonical TOML, before every commit
rm -rf dist && archival build .   # before the checklist: a build removes nothing
```

`archival run . -p 1024` rebuilds on change and serves `http://localhost:1024`.
Run it in the background and tell the person that address is theirs alone;
nothing is public until F. Never edit `dist/`.

## E. Media

Images, video, audio and downloads are uploads: never files in the repo, never
in `git add`. A file field holds `sha`, `filename`, `mime` and `display_type`
(`image`, `video`, `audio` or `upload`), and archival renders the URL as
`{uploads_url}/{upload_prefix}{sha}/{filename}`.

**Before the site is claimed** there is no hosted site to upload to, so media
goes through the preview (F.2 gives you the token file):

```bash
sha=$(shasum -a 256 hero.jpg | awk '{print $1}')
curl -sS -f -X PUT \
  -H "Authorization: Bearer $(sed -n 's/.*"token":"\([^"]*\)".*/\1/p' "$HOME/.archival/preview.json")" \
  --data-binary "@hero.jpg" "https://api.archival.dev/previews/self-serve/upload/$sha/hero.jpg"
```

Then put the pairing's `uploadsUrl` and `uploadPrefix` into `archival.toml` as
`uploads_url` and `upload_prefix`, so `archival build` renders real URLs, and
write the block by hand:

```toml
[hero_image]                 # or [[section.image]] inside a child list
sha = "<the sha you uploaded>"
filename = "hero.jpg"
mime = "image/jpeg"
display_type = "image"
```

Until an upload exists, leave the field out of the object and let templates
guard with `{% if site.hero_image != blank %}`.

**After the claim**, `archival upload` does all of it — the hash, the upload and
the block — against the real site:

```bash
archival upload post/hello cover ./hero.jpg     # a list item
archival upload site og_image ./card.png        # a root object
```

It needs `logged_in=yes` and a remote it can read (`remote=archival` or
`remote=github`), or `-r domain/<domain>`. The claim copies every preview upload
under the site's own prefix, so blocks written before the claim keep working.

The manifest's `uploads_url` and `upload_prefix` are preview values before the
claim and platform values after it; the reset in F.7 is what switches them.
Never hand-edit them once the folder is linked.

## F. Going live for a new site

1. **Say it plainly first**: the preview is public, every publish is reviewed,
   it expires if nothing is done with it, and claiming it is how they keep it.
   Payment happens in their browser, and you cannot do that step.

2. **Pair** (the default, when no `Session:` arrived). The token lands in a
   file, not a variable:

   ```bash
   API=https://api.archival.dev
   curl -sS -X POST "$API/previews/self-serve/pair/start" \
     -H 'Content-Type: application/json' -d '{"slug":"<site name>"}'
   # -> {"code":"K7QM…","verifyUrl":"https://archival.dev/link?c=K7QM…","expiresIn":600}
   ```

   Send one message: the `verifyUrl`, the code, and a request to check that the
   code on the page matches before approving. Then stop and wait. When they
   say they have approved:

   ```bash
   mkdir -p "$HOME/.archival" && (umask 077; curl -sS -o "$HOME/.archival/preview.json" \
     -w '%{http_code}\n' -X POST -d '<CODE>' "$API/previews/self-serve/pair/poll")
   ```

   `204` means not yet. `200` wrote `{name, token, url, uploadsUrl,
   uploadPrefix, …}` to the file, and everything after this reads the token
   from it:

   ```bash
   AUTH="Authorization: Bearer $(sed -n 's/.*"token":"\([^"]*\)".*/\1/p' "$HOME/.archival/preview.json")"
   curl -sS -H "$AUTH" "$API/previews/self-serve/status"
   ```

   Then ship the folder and build, once:

   ```bash
   find . -type f -not -path './dist/*' -not -path './.git/*' -not -path './.archival-bin/*' \
     -not -path './node_modules/*' -not -path './.github/*' -not -name .DS_Store \
     | sed 's|^\./||' \
     | while read -r f; do
         curl -sS -f -X PUT -H "$AUTH" --data-binary "@$f" \
           "$API/previews/self-serve/file/source/$f" >/dev/null || echo "failed: $f"
       done
   curl -sS -X POST -H "$AUTH" "$API/previews/self-serve/build"
   ```

   A `422` is archival's own diagnostic from the server's pinned version. When
   the same source builds locally, the message names the difference; change the
   template rather than chasing the version. `reference/publishing.md` has the
   limits and the error table.

3. **MCP variant.** When a `Session:` arrived with the prompt, or the
   `archival_*` tools are connected and pairing is refused, use them instead:
   `archival_write_files` in batches with the same exclusions,
   `archival_upload_media` for media, one `archival_publish`. `archival_status`
   plays the part of `status` below.

4. **Hand over the `url`**, on its own line. Never `siteUrl`.

5. **They claim it** at that link: Go Live, sign in, choose a domain, pay. The
   claim copies the preview into a new repository, rewrites `archival.toml`,
   adds `.github/workflows/archival.yml`, and that first commit deploys the site.

6. **`archival login`.** Ask them to run it in their own terminal, or run it in
   the background and relay the URL it prints; it blocks until the browser
   finishes. Re-run `site-info.sh` until `logged_in=yes`. From here every
   account call reads the token like this, inside the command and never echoed:

   ```bash
   TOKEN_HDR="Authorization: Bearer $(sed -n 's/^access_token *= *"\(.*\)"/\1/p' "$HOME/.archivalrc")"
   ```

7. **Find the site and link the folder.** The preview knows what it became:

   ```bash
   curl -sS -H "$AUTH" "$API/previews/self-serve/status"
   # -> …, "claimed": {"owner":"<owner>","name":"<name>","domain":"<domain>"}   (null until they claim)
   ```

   Setup runs in the background on our side, so poll the site until it is ready:

   ```bash
   curl -sS -H "$TOKEN_HDR" "$API/sites/github/<owner>/<name>"
   ```

   Ready means a 200 whose `setupInfo.repo` is `{"Success": "<sha>"}` and whose
   `domains` is non-empty; a 404 or 403 means wait a few seconds and ask again.
   Then mint a remote and point the folder at it. The folder must be clean
   (`dirty=no`) — commit or discard first — because the reset replaces local
   history with the site's.

   ```bash
   curl -sS -H "$TOKEN_HDR" -H 'Content-Type: application/json' -X POST \
     "$API/git-remote/github/<owner>/<name>" -d '{"label":"<machine name> cli","domain":"<domain>"}'
   # -> {"url":"https://archival:git-…@git.archival.dev/<domain>","token":{…}}
   git remote add origin '<url>'        # or: git remote set-url origin '<url>'
   git fetch origin
   git branch -M main
   git reset --hard origin/main
   git branch --set-upstream-to=origin/main main
   archival build .
   ```

   `git diff ORIG_HEAD --stat` shows what the platform changed: `archival.toml`
   (now carrying `site_url`, `upload_prefix`, `uploads_url` and
   `metadata.source_preview`) and the new workflow file. Everything else is what
   you published. Tell the person the remote URL embeds a 90-day secret that
   now sits in `.git/config`, revocable from the site's settings or with
   `DELETE /git-remote/github/<owner>/<name>`.

   A 401 from `status` means the two-hour preview grant has ended. Ask which
   domain they chose and find the site in `GET /sites` by `domains[].name`.

8. **From now on**: edit → `archival build .` → `git commit` → `git push`
   deploys; `archival upload` for media. Deploy progress is `setupInfo.deploy`
   on `GET /sites/github/<owner>/<name>`.

## G. Linking a folder to an existing site

For a site the person already has, or one they just created at
`editor.archival.dev/new`:

```bash
curl -sS -H "$TOKEN_HDR" "$API/sites"
```

Show them each site's `domains[0].name` and ask which. Mint a remote as in F.7.
Then:

- **An empty folder**: `git clone '<url>' .` and `archival build .`. The cloned
  manifest already carries `upload_prefix` and `site_url`, so images resolve.
- **A folder holding the real content, and a blank hosted site**: add the
  remote, `git fetch origin`, `git reset --soft origin/main`, then
  `git checkout HEAD -- archival.toml .github` to keep the platform's manifest
  and workflow, put your `site_name` (and `prebuild`, if any) back into
  `archival.toml`, commit, and `git push -u origin main`.
- **`remote=github` already, and the site lists that repo**: nothing to mint.
  `git push` deploys when `workflow=yes`.

A push over 100 MB fails at the edge with no useful message. Media never goes
through git.

## H. What to tell the person, and when

| Step | Say |
|---|---|
| Install | Nothing. It is a tool on this machine, not part of their site. |
| Building | "Nothing is public yet; `http://localhost:1024` is only on this computer." |
| Before publishing | Public, reviewed, expires, claim to keep. |
| The link | The `url`, alone on a line. |
| Claiming | "Payment happens in your browser; I cannot do that step." |
| Login | "Open this URL to sign in; the terminal is waiting on it." |
| Linked | "This folder now deploys when I push. Media goes up with `archival upload`, never into git." |
