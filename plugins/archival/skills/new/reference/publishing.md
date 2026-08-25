# Publishing a preview to archival.dev

Three steps: upload the source, upload the built `dist/`, publish. Every request
carries the publish token.

```bash
API=https://api.archival.dev
AUTH="Authorization: Bearer $ARCHIVAL_PUBLISH_TOKEN"
```

The token names the preview. **You never send the preview name** — the server
resolves it from the token, so a token cannot touch anyone else's site.

## 0. No token? Pair for one

Skip this if a token arrived with the request.

```bash
# 1. ask for a code
curl -sS -X POST "$API/previews/self-serve/pair/start"
# -> { "code": "K7QM…", "verifyUrl": "https://archival.dev/link?c=K7QM…", "expiresIn": 600 }

# 2. show the person the URL and the code, then poll until this stops being 204
curl -sS -X POST -d "$CODE" "$API/previews/self-serve/pair/poll"
```

`204` means keep waiting. `200` returns the same body as a launched request —
token, preview name, uploads URL and prefix, archival version. `404` means the
code expired or was already collected.

The code is single-use, expires in ten minutes, and only a person clearing a
challenge in a browser can approve it.

## 1. Check what you have

```bash
curl -sS -H "$AUTH" "$API/previews/self-serve/status"
```

```json
{
  "name": "blue-fig-bakery-a1b2c3d4",
  "url": "https://blue-fig-bakery-a1b2c3d4.preview.onarchival.dev",
  "published": false,
  "publishes": 0,
  "publishesRemaining": 20,
  "hasSource": false,
  "expiresAt": "2026-08-22T19:04:11.000Z"
}
```

Run this first. It confirms the token works before you spend time uploading, and
an expired token is much clearer here than midway through a hundred files.

## 2. Upload

One request per file, raw bytes in the body, path in the URL:

```
PUT /previews/self-serve/file/<kind>/<path>
```

`<kind>` is `source` for the site source and `dist` for the built output. Upload
**both**:

- **`dist`** is what gets served. Without it there is nothing to show.
- **`source`** is what lets the person keep the site — claiming a preview opens
  its source in the Archival editor. Skip it and they can look but never own it.

```bash
cd <site dir>

# source: everything except build output and version control
find . -type f \
  -not -path './dist/*' -not -path './.git/*' -not -path './.archival-bin/*' \
  | sed 's|^\./||' \
  | while read -r f; do
      curl -sS -f -X PUT -H "$AUTH" --data-binary "@$f" \
        "$API/previews/self-serve/file/source/$f" >/dev/null
    done

# dist: the built site
cd dist && find . -type f | sed 's|^\./||' | while read -r f; do
  curl -sS -f -X PUT -H "$AUTH" --data-binary "@$f" \
    "$API/previews/self-serve/file/dist/$f" >/dev/null
done
```

Uploads are idempotent — re-uploading a path replaces it. To remove a file that
a later edit deleted:

```bash
curl -sS -f -X DELETE -H "$AUTH" "$API/previews/self-serve/file/dist/old-page.html"
```

Paths must be relative; absolute paths, `..`, and empty segments are rejected.

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

`start` gives you `uploadsUrl` and `uploadPrefix`; put both in `archival.toml`:

```toml
uploads_url = "https://preview-uploads.archival.dev"
upload_prefix = "blue-fig-a1b2c3d4/"
```

An `image` field then resolves to
`{uploads_url}/{upload_prefix}{sha}/{filename}` on its own — you write the
`sha`, `filename` and `mime` into the object file and archival builds the URL:

```toml
[[section.image]]
sha = "<the sha you uploaded>"
filename = "hero.jpg"
mime = "image/jpeg"
display_type = "image"
```

Upload the file **before** you build, or the build renders a URL to something
that is not there yet.

You do not need to set `Content-Type`. What a file is served as is decided from
its extension, so `curl --data-binary` (which would otherwise send
`application/x-www-form-urlencoded` and make every page download instead of
render) needs no special handling.

## 3. Publish

```bash
curl -sS -f -X POST -H "$AUTH" "$API/previews/self-serve/publish"
```

```json
{
  "url": "https://blue-fig-bakery-a1b2c3d4.preview.onarchival.dev",
  "publishes": 1,
  "publishesRemaining": 19
}
```

The URL is live immediately — the site is served straight from the uploaded
`dist`, with no build step on the server. Publishing again after edits is the
normal loop; upload the changed files and call publish again.

## Limits

| | |
|---|---|
| Token lifetime | ~2 hours |
| Publishes per token | 20 |
| Files | 500 per kind |
| Single file | 10 MB |
| Total | 25 MB of site files, 100 MB of uploads |

A preview with no traffic is deleted after 30 days.

## Errors

| Status | Meaning |
|---|---|
| 400 | Publishing before any `dist` file was uploaded. Upload, then publish. |
| 401 | Missing, malformed, expired, or unknown token. Get a fresh link. |
| 403 | Path rejected (absolute, or containing `..` or an empty segment). |
| 404 | Unknown `<kind>`. It is `source` or `dist`, nothing else. |
| 413 | File or preview over the size limit. |
| 429 | Rate limited, or publishes exhausted. |

Only 429 is worth retrying, and only after a wait. The rest are all mistakes in
the request: fix the path, the kind, the order, or get a fresh link.

## What to tell the person

Say these plainly before the first publish:

- The URL is **public**. Anyone with it can see the site.
- Every publish is **reported to Archival** for abuse review.
- The preview **expires** if they do nothing with it.
- To keep it, open the URL and choose to claim the site.

Never publish content they did not ask for, and never publish a site containing
invented facts about a real business or person.
