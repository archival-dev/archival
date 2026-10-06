# Conventions: a site the person can keep

An Archival site is only as good as what its owner can change without you.
These rules are what the Archival editor, the hosted platform and the machines
that read websites expect of a site. They apply in a chat and in a folder
alike; §11 is the check to run against `dist/` after `archival build`.

## 1. Content lives in objects

A name, a price, a paragraph, an address, opening hours or a phone number in a
`.liquid` file is something the person cannot change later. Everything a human
would ever edit is a field on an object; templates only arrange fields.

The exceptions are copy that belongs to the design: the 404 page's text and the
footer's "Powered by Archival". Hardcoded URLs, API keys and example addresses
are never acceptable in a template. They ship to every visitor and survive
every edit.

Checks:

```bash
# long literal text in templates: every hit should be the 404 page or the footer
grep -En '>[^<{%]{60,}<' pages/*.liquid layout/*.liquid
# the business name appears only in content
grep -rn "<the business name>" pages layout public
```

## 2. One identity root object

`[site]` (singular) holds what the layout needs everywhere: `name`,
`description`, `og_image = "image"`, and whatever else the design reads
(`logo`, `tagline`, `email`, `phone`, `theme_color`). `site_name` in
`archival.toml` is **not** a template variable; the layout reads `site.name`.

## 3. Lists for anything there is more than one of

- A **list type** for every plural: posts, events, products, projects, team
  members, testimonials, locations. One file per item under `objects/<type>/`,
  each with an `order` field.
- `template = "<type>"` on every type a visitor clicks through to. Each item
  renders at `/<type>/<filename>.html` and is linked as `/{{ item.path }}.html`.
- **Child collections** (`[type.child]` in the schema, `[[child]]` in the data)
  for list-shaped data inside an object: tags, hours, FAQ entries, links. Never
  a comma-separated string.
- **Real types**: `boolean` for a toggle, an inline array for a fixed choice,
  `date` for a date, `image`/`video`/`audio`/`upload` for a file, `markdown` for
  prose, `number` for a number.
- **`show_<section>` booleans** on the parent for sections a person may want to
  hide, read as `{% if site.show_hours != false %}` so an absent field means
  shown.
- **Singular type names** for roots and lists: `post`, `event`, `site`, never
  `posts`. Lists pluralize themselves in templates, and a plural root is not
  addressable by its own name.

Check: `grep -E '^\[[a-z_]+s\]' archival_objects.toml` and review every hit.

## 4. `archival_editor.toml`

The editor reads this file for how to show each list. Every list type and every
nested child list gets a view; the one or two lists the person will add to most
get a shortcut.

```toml
[[event.views]]
name = "default"
primary = "title"
secondary = "date"

[[post.views]]
name = "default"
primary = "title"
secondary = "publish_date"

[[shortcuts]]
name = "New Event"
path = "event"
```

`primary`, `secondary` and `tertiary` are field names of that type, or
`filename`. A shortcut's `path` is the type name for a top-level list, or
`<root>.<child>` for a child list on a root object.

Check:

```bash
for d in objects/*/; do t=$(basename "$d"); grep -q "^\[\[$t.views\]\]" archival_editor.toml || echo "no view: $t"; done
```

## 5. Computer-facing pages

Feed readers, search engines and AI agents read the site too. These are
ordinary Liquid pages whose second extension sets the output type, and every
one of them is generated from the objects: a loop over what it lists, never
text written out by hand.

- `pages/llms.txt.liquid`: **every site gets one**. What the site is, its main
  pages, and its kinds of content, from `site` and the lists. Nothing about how
  it was built.
- `pages/sitemap.xml.liquid` and `pages/robots.txt.liquid`: every page and
  every templated item in the sitemap; robots allows all and names the sitemap
  with `Sitemap: {{ site_url }}/sitemap.xml`.
- `pages/feed.xml.liquid`: whenever there is a dated list. The layout's `<head>`
  then carries
  `<link rel="alternate" type="application/rss+xml" title="<what it carries>" href="{{ site_url }}/feed.xml">`.
- `pages/<type>.json.liquid`: optional, a JSON endpoint for a list other
  software might consume.

`{{ site_url }}` is empty until the site is claimed; absolute links in these
files resolve once it is. That is expected.

Check: `test -f dist/llms.txt && test -f dist/sitemap.xml && test -f dist/robots.txt`;
`grep -q 'rel="alternate"' dist/index.html` when `dist/feed.xml` exists;
`xmllint --noout dist/sitemap.xml dist/feed.xml` where `xmllint` exists.

## 6. Head metadata, on every page

In `layout/theme.liquid`, from the identity object and the page's `title:`:

- `<title>` as `<page title> — <site name>`, or the site name alone on the home page
- `<meta name="description">`
- `og:title`, `og:description`, `og:type`, `og:site_name`, `og:image` (from
  `site.og_image`, falling back to the design's main image) and `twitter:card`
  (`summary_large_image` with an image, else `summary`)
- `<meta name="theme-color">`, ideally light and dark variants
- a favicon: an inline SVG data URL built from the site name's initials needs
  no file

Pass `title:` from every page: `{% layout 'theme' title: "" %}` on the home
page, a literal on section pages, `item.title` on templated pages. A missing
argument fails the build.

## 7. JSON-LD

A `<script type="application/ld+json">` in the `<head>` of the home page and
every templated page, with a schema.org type that fits (`LocalBusiness` or a
subtype such as `Restaurant`, `Organization`, `Person`, `Blog` and
`BlogPosting`, `Event`, `Product`, `Recipe`) and only properties the objects
actually hold. There is no `json` filter; escape strings yourself:

```liquid
{% capture ld_name %}{{ site.name | replace: '\', '\\' | replace: '"', '\"' | strip_newlines }}{% endcapture %}
```

Dates as `| date: "%Y-%m-%d"`, images as `.url`, optional properties inside
`{%- if x != blank -%}` so a blank field leaves no trailing comma.

## 8. Accessibility

- `<html lang="en">`
- a skip link as the first child of `<body>`: `<a class="skip-link" href="#main">Skip to content</a>`
- one `<main id="main">`, in the layout or on every page, never nested
- exactly one `<h1>` per page; a layout masthead demotes itself on subpages
- `alt` on every meaningful image, from a field; `alt=""` on decoration
- `aria-expanded` on anything that toggles; Escape closes what a click opened
- `prefers-reduced-motion` respected in CSS and in any script that moves things
- `rem` for every dimension, never `px`
- text that passes contrast against its background

## 9. Liquid as archival renders it

- `{% if x != blank %}` is the idiom for "is set", not `{% if x %}`.
- Every page passes `title:` (§6).
- Inside a templated page the type's name is the current item; iterate the
  whole list with `objects.<type>`.
- `include` sees the caller's variables and `render` does not; they are not
  interchangeable.
- A file field renders through `.url` (`{{ post.cover.url }}`), never bare.
- An undefined variable fails the build.
- A markdown field's link URLs cannot hold Liquid (they are encoded first); use
  relative paths in content.
- Wrap every markdown render in `<div class="markdown">` and ship a
  `.markdown { … }` block covering headings, lists, code, tables, blockquotes
  and images, at the end of the stylesheet.
- The footer year is `{{ "now" | date: "%Y" }}`, never a field or a literal.
- `{% unless %} … {% else %}` is unreliable; use `if … != blank … else`.

## 10. `archival.toml` for a real site

Set `site_name`. Browser scripts go in `scripts/`, which archival builds into
`/js/` with no build step: `scripts/main.ts` is served as `/js/main.js` with its
types stripped, and `.js` files are copied as they are. Only erasable TypeScript
compiles (no `enum`, `namespace` or parameter properties), nothing is bundled, and
a relative import may name either `./util.ts` or `./util.js`. Set `prebuild` only
for any other build step (its outputs go to `public/`, never `dist/`). Do **not** set `site_url`, `upload_prefix`,
`uploads_url` or `metadata`: the platform writes them when the site is claimed,
and a preview pairing sets the upload keys for the meantime. Gitignore `dist/`.

## 11. The checklist

Run after `rm -rf dist && archival build .`, fix every line it prints, and build
again:

```bash
test -f dist/llms.txt || echo "no llms.txt"
test -f dist/sitemap.xml || echo "no sitemap.xml"
test -f dist/robots.txt || echo "no robots.txt"
[ -f dist/feed.xml ] && { grep -q 'rel="alternate"' dist/index.html || echo "feed not linked from layout"; }
for f in $(find dist -name '*.html'); do
  for needle in '<title>' 'name="description"' 'property="og:title"' 'lang="en"' 'id="main"'; do
    grep -q "$needle" "$f" || echo "missing $needle: $f"
  done
  [ "$(grep -c '<h1' "$f")" = 1 ] || echo "h1 count is not 1: $f"
  grep -q 'href="#"' "$f" && echo "href=\"#\": $f"
  grep -q 'onclick=' "$f" && echo "onclick: $f"
done
for f in dist/index.html dist/*/*.html; do
  [ -f "$f" ] && { grep -q 'application/ld+json' "$f" || echo "no JSON-LD: $f"; }
done
for d in objects/*/; do t=$(basename "$d"); grep -q "^\[\[$t.views\]\]" archival_editor.toml 2>/dev/null || echo "no editor view: $t"; done
grep -En '>[^<{%]{60,}<' pages/*.liquid layout/*.liquid
```

The last line is for your judgement: every hit should be the 404 page or the
footer.
