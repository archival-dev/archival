# Publishing a preview to archival.dev, without the MCP tools

This is the same flow the `archival_*` tools drive, over plain HTTP. Use it when
the tools are not connected. If they are, use them — they do all of this.

```bash
API=https://api.archival.dev
AUTH="Authorization: Bearer $ARCHIVAL_PUBLISH_TOKEN"
```

The token names the preview. **You never send the preview name** — the server
resolves it from the token, so a token cannot touch anyone else's site.

## 0. Get a token

Skip this if one arrived with the request.

```bash
# 1. ask for a code
curl -sS -X POST "$API/previews/self-serve/pair/start" \
  -H 'Content-Type: application/json' -d '{"slug":"blue fig bakery"}'
# -> { "code": "K7QM…", "verifyUrl": "https://archival.dev/link?c=K7QM…", "expiresIn": 600 }

# 2. show the person the URL and the code, then poll until this stops being 204
curl -sS -X POST -d "$CODE" "$API/previews/self-serve/pair/poll"
```

`204` means keep waiting. `200` returns the token, preview name, uploads URL and
prefix. `404` means the code expired or was already collected.

The code is single-use, expires in ten minutes, and only a person clearing a
challenge in a browser can approve it. The `slug` is optional and names the
preview's subdomain.

## 1. Check what you have

```bash
curl -sS -H "$AUTH" "$API/previews/self-serve/status"
```

```json
{
  "name": "blue-fig-bakery-a1b2c3d4",
  "url": "https://editor.archival.dev/preview?n=blue-fig-bakery-a1b2c3d4",
  "siteUrl": "https://blue-fig-bakery-a1b2c3d4.preview.onarchival.dev",
  "published": false,
  "publishes": 0,
  "publishesRemaining": 20,
  "hasSource": false,
  "expiresAt": "2026-08-22T19:04:11.000Z"
}
```

Run this first. It confirms the token works before you spend time uploading, and
an expired token is much clearer here than midway through a hundred files.

## 2. Upload the source

One request per file, raw bytes in the body, path in the URL:

```bash
cd <site dir>

find . -type f \
  -not -path './dist/*' -not -path './.git/*' -not -path './.archival-bin/*' \
  | sed 's|^\./||' \
  | while read -r f; do
      curl -sS -f -X PUT -H "$AUTH" --data-binary "@$f" \
        "$API/previews/self-serve/file/source/$f" >/dev/null
    done
```

Uploads are idempotent — re-uploading a path replaces it. To remove a file a
later edit deleted:

```bash
curl -sS -f -X DELETE -H "$AUTH" "$API/previews/self-serve/file/source/old-page.liquid"
```

Paths must be relative; absolute paths, `..`, and empty segments are rejected.
A source tree containing `carriers/` is refused at build time.

You do not need to set `Content-Type`. What a file is served as is decided from
its extension, so `curl --data-binary` (which would otherwise send
`application/x-www-form-urlencoded` and make every page download instead of
render) needs no special handling.

### Media does not go here

Images, video, audio and other binaries are **uploads**, not site files. They are
content-addressed and served from a CDN, which is why an Archival site never
carries them in its source:

```
PUT /previews/self-serve/upload/<sha256>/<filename>
```

`<sha256>` is the SHA-256 of the file, lowercase hex, and the server verifies it
— a wrong one is a 400. Re-uploading bytes that are already there returns `200`
with `"existed": true` and writes nothing.

```bash
sha=$(shasum -a 256 hero.jpg | awk '{print $1}')
curl -sS -f -X PUT -H "$AUTH" --data-binary "@hero.jpg" \
  "$API/previews/self-serve/upload/$sha/hero.jpg"
```

Pairing gives you `uploadsUrl` and `uploadPrefix`; put both in `archival.toml`:

```toml
uploads_url = "https://preview-uploads.archival.dev"
upload_prefix = "blue-fig-a1b2c3d4/"
```

A file field then resolves to `{uploads_url}/{upload_prefix}{sha}/{filename}` on
its own — you write the `sha`, `filename` and `mime` into the object file and
archival builds the URL:

```toml
[[section.image]]
sha = "<the sha you uploaded>"
filename = "hero.jpg"
mime = "image/jpeg"
display_type = "image"
```

Upload the file **before** you publish, or the build renders a URL to something
that is not there yet.

## 3. Build and publish

```bash
curl -sS -f -X POST -H "$AUTH" "$API/previews/self-serve/build"
```

```json
{
  "url": "https://editor.archival.dev/preview?n=blue-fig-bakery-a1b2c3d4",
  "siteUrl": "https://blue-fig-bakery-a1b2c3d4.preview.onarchival.dev",
  "publishes": 1,
  "publishesRemaining": 19
}
```

`url` is the link to give the person, and the only one to give them: it frames
the site with the notice and the path to claiming it. `siteUrl` is the raw
origin the site is served from — use it to fetch a built page and check
something, never to share.

The server builds the source you uploaded with a pinned `archival` binary and
serves the result. You do not need the CLI, and you do not upload `dist/`. A
build failure comes back as `422` with archival's own diagnostic in the body —
fix what it names and call this again.

Every build replaces the served site wholesale, so a page you deleted from the
source stops being served. Publishing again after edits is the normal loop:
upload the changed files and call build again.

## Limits

|                     |                                          |
| ------------------- | ---------------------------------------- |
| Token lifetime      | ~2 hours                                 |
| Publishes per token | 20                                       |
| Files               | 500                                      |
| Single file         | 10 MB                                    |
| Total               | 25 MB of site files, 100 MB of uploads   |

A preview with no traffic is deleted after 30 days.

## Errors

| Status | Meaning                                                            |
| ------ | ------------------------------------------------------------------ |
| 400    | Building with no source uploaded, or a source tree with `carriers/`. |
| 401    | Missing, malformed, expired, or unknown token. Get a fresh link.    |
| 403    | Path rejected (absolute, or containing `..` or an empty segment).  |
| 413    | File or preview over the size limit.                               |
| 422    | The build failed. The body is archival's error — fix it and retry. |
| 429    | Rate limited, or publishes exhausted.                              |

429 is worth retrying after a wait, and 422 after fixing what it named. The rest
are mistakes in the request: fix the path, the order, or get a fresh link.

## What to tell the person

Say these plainly before the first publish:

- The site is **public**. Anyone with the link can see it.
- Every publish is **reported to Archival** for abuse review.
- The preview **expires** if they do nothing with it.
- To keep it, open the link and choose to claim the site.

Never publish content they did not ask for, and never publish a site containing
invented facts about a real business or person.
