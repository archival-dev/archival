//! The sitemap.xml and robots.txt a build writes for a site that has a
//! `site_url` and whose pages write neither.

use crate::util::path_to_slash;
use std::path::Path;

pub(crate) const SITEMAP_FILE_NAME: &str = "sitemap.xml";
pub(crate) const ROBOTS_FILE_NAME: &str = "robots.txt";

/// A sitemap of every HTML page in `outputs`, which are relative to the build
/// dir, each at the extensionless URL it is served from.
pub(crate) fn urlset<'a>(site_url: &str, outputs: impl IntoIterator<Item = &'a Path>) -> String {
    let base = site_url.trim_end_matches('/');
    let mut paths: Vec<String> = outputs.into_iter().filter_map(served_path).collect();
    paths.sort();
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for path in paths {
        let loc = escape_xml(&format!("{base}/{path}"));
        xml.push_str(&format!("  <url><loc>{loc}</loc></url>\n"));
    }
    xml.push_str("</urlset>\n");
    xml
}

pub(crate) fn robots(site_url: &str) -> String {
    format!(
        "User-agent: *\nAllow: /\n\nSitemap: {}/{SITEMAP_FILE_NAME}\n",
        site_url.trim_end_matches('/')
    )
}

/// The path, without its leading `/`, that a page built to `output` is served
/// at: `about.html` at `about`, `blog/index.html` at `blog/`. `None` for the 404
/// page and for anything that is not HTML.
fn served_path(output: &Path) -> Option<String> {
    if output.extension()? != "html" {
        return None;
    }
    let page = path_to_slash(output.with_extension(""));
    if page == "404" {
        return None;
    }
    let page = match page.strip_suffix("index") {
        Some(dir) if dir.is_empty() || dir.ends_with('/') => dir.to_string(),
        _ => page,
    };
    Some(
        page.split('/')
            .map(urlencoding::encode)
            .collect::<Vec<_>>()
            .join("/"),
    )
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn pages_are_listed_where_they_are_served() {
        for (output, served) in [
            ("index.html", Some("")),
            ("about.html", Some("about")),
            ("post/a-post.html", Some("post/a-post")),
            ("blog/index.html", Some("blog/")),
            ("reindex.html", Some("reindex")),
            ("post/a post.html", Some("post/a%20post")),
            ("404.html", None),
            ("llms.txt", None),
            ("sitemap.xml", None),
            ("js/main.js", None),
        ] {
            assert_eq!(
                served_path(Path::new(output)).as_deref(),
                served,
                "{output}"
            );
        }
    }

    #[test]
    fn a_urlset_is_sorted_and_absolute() {
        let outputs = ["post/b.html", "about.html", "index.html", "404.html"];
        assert_eq!(
            urlset("https://example.com/", outputs.iter().map(Path::new)),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n  \
             <url><loc>https://example.com/</loc></url>\n  \
             <url><loc>https://example.com/about</loc></url>\n  \
             <url><loc>https://example.com/post/b</loc></url>\n\
             </urlset>\n"
        );
    }

    #[test]
    fn locations_are_escaped_for_xml() {
        assert_eq!(
            escape_xml(r#"https://example.com/a&b<c>"d'"#),
            "https://example.com/a&amp;b&lt;c&gt;&quot;d&apos;"
        );
    }

    #[test]
    fn robots_names_the_sitemap() {
        assert_eq!(
            robots("https://example.com/cookbook/"),
            "User-agent: *\nAllow: /\n\nSitemap: https://example.com/cookbook/sitemap.xml\n"
        );
    }
}
