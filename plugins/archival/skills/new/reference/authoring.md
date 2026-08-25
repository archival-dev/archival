# Authoring an Archival site

Content is TOML, presentation is Liquid. Read this before writing either.

Full docs: <https://archival.dev/docs/llms.txt>. Schemas (CORS-open, and also at
the archival repo root):
<https://archival.dev/schemas/archival_objects.schema.json>.

## Layout of a site

| Path | What it is | Manifest key |
|---|---|---|
| `archival.toml` | Site config. May be empty. | — |
| `archival_objects.toml` | The content schema — every object type. | `object_definition_file` |
| `objects/` | The content. | `objects` |
| `pages/` | Liquid page templates. | `pages` |
| `layout/` | Shared page chrome. **Singular.** | `layout_dir` |
| `public/` | Copied verbatim into the build. CSS, fonts, images. | `static_dir` |
| `dist/` | Build output. Gitignore it. | `build_dir` |

`archival.toml` usually needs only:

```toml
site_name = "Blue Fig Bakery"
site_url = "https://example.com"
```

## Object types

`archival_objects.toml` maps field names to type names. A type is either a
**root object** (one `objects/<type>.toml`) or a **list** (a directory
`objects/<type>/*.toml`, one file per item).

```toml
# a root object: objects/home.toml
[home]
tagline = "string"
intro = "markdown"

# a list, one file per item, each rendered by pages/post.liquid
[post]
title = "string"
publish_date = "date"
body = "markdown"
template = "post"
```

`template = "post"` makes each object render through `pages/post.liquid` into
`dist/post/<filename>.html`. Without it a type is data you loop over, and no
per-object page is generated.

### Field types

| Type | Notes |
|---|---|
| `string` | Plain text. Field values are themselves rendered as Liquid. |
| `markdown` | Parsed to HTML (GFM + footnotes, description lists, superscript, header IDs). **Raw HTML passes through unsanitized.** |
| `number` | Always a signed 64-bit float. Use `\| round: 2`. |
| `boolean` | Unquoted `true` / `false`. |
| `date` | `YYYY-MM-DD`, `YYYY-MM-DD HH:MM:SS`, `MM/DD/YYYY HH:MM:SS`, and more; `now` / `today` resolve at build. Render with `\| date: "%B %-d, %Y"`. |
| `image`, `video`, `audio`, `upload` | CDN-backed. Written by the editor or `archival upload`, **not by hand.** |
| `secret` | Never enters a template — referencing one *fails the build*. Not for static sites. |
| `["a", "b", "c"]` | An enum: an inline array of allowed strings. |

**Media is uploaded, never committed.** An `image`/`video`/`audio`/`upload`
field carries a `sha`, `filename` and `mime`, and archival resolves the URL as
`{uploads_url}/{upload_prefix}{sha}/{filename}`. Upload the file first and use
its real SHA-256 — an invented one renders a URL to nothing. See
`reference/publishing.md`.

`public/` is for things that are part of the design rather than the content:
stylesheets, fonts, favicons, an SVG logo.

### Child fields

A nested table gives an object a repeating list:

```toml
[home]
tagline = "string"
[home.section]
heading = "string"
body = "markdown"
```

```toml
# objects/home.toml
tagline = "Sourdough, daily."

[[section]]
heading = "Our bread"
body = "Milled locally, fermented overnight."

[[section]]
heading = "Visit"
body = "Open Tuesday to Sunday."
```

```liquid
{% for section in home.section %}
  <section><h2>{{ section.heading }}</h2>{{ section.body }}</section>
{% endfor %}
```

## Templates

Every page in `pages/` becomes an HTML file: `pages/index.liquid` →
`dist/index.html`. Files prefixed with `_` are partials and render no page.

### Built-in variables

- `objects` — every type, keyed by name.
- **Inflected accessors** — the accessor is the type name run through an
  inflector: a list is **pluralized** (`post` → `posts`), a root object is
  **singularized** (`settings` → `setting`, `home` → `home`). A name that is
  already the right number comes back unchanged, which is what makes this easy
  to miss. `objects.<name>` always takes the name exactly as written, and is the
  way out when an inflection surprises you.
- `site_url` — from `archival.toml`.
- `page` — the name of the page being rendered.

On a `template` page the current object is bound to its type name, and carries
`object_name`, `order`, and `path` (`post/my-first-post`).

```liquid
{% for post in posts %}
  <a href="/{{ post.path }}.html">{{ post.title }}</a>
{% endfor %}
```

### Layouts

`{% layout %}` is archival-specific and takes named arguments. The layout
receives them plus `page_content`, which holds the whole calling document.

```liquid
{% layout 'theme' title: post.title %}
<article><h1>{{ post.title }}</h1>{{ post.body }}</article>
```

```liquid
<!-- layout/theme.liquid -->
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>{{ title }}</title>
    <link rel="stylesheet" href="/style.css" />
  </head>
  <body>
    {{ page_content }}
  </body>
</html>
```

### Partials

`_video.liquid` is included as `'video'`; `pages/partials/_card.liquid` as
`'partials/card'`.

```liquid
{% include 'card', title: post.title, body: post.body %}
```

`include` sees the caller's variables; `render` is scope-isolated and sees only
its arguments (plus the globals above). They are **not** interchangeable —
swapping `include` for `render` silently breaks a partial that reads a caller
local.

## Gotchas that cost real time

- **A plural root object is not addressable by its own name.** `[settings]` in
  the schema is `setting` in a template, so `{{ settings.title }}` fails the
  build as an unknown variable. Name root object types in the singular — `site`,
  `home`, `about` — and lists in the singular too, since they pluralize
  themselves.
- **`site_name` is not a template variable.** `archival.toml` takes both
  `site_name` and `site_url`, but only `site_url` is exposed to templates.
  Referencing `site_name` fails the build with `liquid: failed to evaluate
  value`. Put the site's name in an object field, or write it literally.
- **Field values render as Liquid.** A literal `{{` in content needs escaping.
- **A page may render twice.** If output still contains Liquid after the first
  pass, archival renders again — so a bare `{% raw %}` does not survive into
  output.
- **`layout/` is singular**, and the build dir is `dist`, the static dir
  `public`. Don't guess these from other generators.
- **Everything in `public/` is copied verbatim**, preserving subpaths, so
  `public/style.css` is served at `/style.css`.
- **`archival build` is the source of truth.** A schema or template error fails
  it. Never claim a site works without a clean build.

## Useful commands

```bash
archival build .              # render into dist/
archival run .                # rebuild on change, serve on :1024 (local only)
archival objects .            # list this site's objects
archival schemas . --inline   # JSON Schema for this site's own object types
archival format .             # canonical TOML formatting
```
