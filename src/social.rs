//! What the social web has done with a site, handed to a build as a snapshot and
//! rendered under the `archival` global.
//!
//! The snapshot's shape is the carrier API's: `archival.activitypub`,
//! `archival.atproto`, `archival.webmentions` and `archival.interactions` hold
//! what `site.activitypub`, `site.atproto`, `site.webmentions` and
//! `site.interactions` answer, keyed by an object's `path` (or a page path for
//! webmentions). The file is camelCase like a carrier response; templates read
//! the same keys in snake_case. Without a snapshot every value is the carrier's
//! empty answer, which is also what a template reads for a path nothing has
//! interacted with.

use crate::liquid_kstring::KStringCow;
use liquid::{ObjectView, ValueView};
use liquid_core::Value;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};

/// The global the snapshot renders under.
pub const ARCHIVAL_GLOBAL: &str = "archival";

macro_rules! carrier_type {
    ($(#[$meta:meta])* pub struct $name:ident { $($body:tt)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all(deserialize = "camelCase", serialize = "snake_case"))]
        pub struct $name { $($body)* }
    };
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CarrierPage<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub next: Option<String>,
}

impl<T> Default for CarrierPage<T> {
    fn default() -> Self {
        Self {
            items: vec![],
            total: 0,
            next: None,
        }
    }
}

carrier_type! {
    pub struct SocialSnapshot {
        #[serde(default)]
        pub generated_at: Option<String>,
        #[serde(default)]
        pub activitypub: ActivityPubSnapshot,
        #[serde(default)]
        pub atproto: AtprotoSnapshot,
        #[serde(default)]
        pub webmentions: WebmentionsSnapshot,
        #[serde(default)]
        pub interactions: BTreeMap<String, PostInteractions>,
    }
}

carrier_type! {
    pub struct ActivityPubSnapshot {
        #[serde(default)]
        pub account: Option<ActivityPubAccount>,
        #[serde(default)]
        pub accounts: Vec<ActivityPubAccount>,
        #[serde(default)]
        pub followers: CarrierPage<Follower>,
        #[serde(default)]
        pub posts: BTreeMap<String, ActivityPubPost>,
    }
}

carrier_type! {
    pub struct ActivityPubAccount {
        pub handle: String,
        pub id: String,
        pub url: String,
        pub followers: u64,
        pub following: u64,
        pub posts: u64,
    }
}

carrier_type! {
    pub struct FediverseAccount {
        pub id: String,
        pub handle: Option<String>,
        pub name: Option<String>,
        pub url: Option<String>,
        pub icon: Option<String>,
    }
}

carrier_type! {
    pub struct Follower {
        pub account: FediverseAccount,
        pub follows: String,
        pub followed_at: String,
    }
}

carrier_type! {
    pub struct ActivityPubPost {
        pub counts: ActivityPubCounts,
        pub likes: CarrierPage<Reaction>,
        pub boosts: CarrierPage<Reaction>,
        pub replies: CarrierPage<Reply>,
    }
}

carrier_type! {
    pub struct ActivityPubCounts {
        pub likes: u64,
        pub boosts: u64,
        pub replies: u64,
    }
}

carrier_type! {
    pub struct Reaction {
        pub account: FediverseAccount,
        pub at: String,
    }
}

carrier_type! {
    pub struct ReplyAttachment {
        pub url: String,
        pub media_type: Option<String>,
        pub description: Option<String>,
    }
}

carrier_type! {
    pub struct Reply {
        pub id: String,
        pub url: Option<String>,
        pub in_reply_to: String,
        pub account: FediverseAccount,
        pub content: String,
        pub summary: Option<String>,
        pub sensitive: bool,
        pub attachments: Vec<ReplyAttachment>,
        pub published: String,
        pub updated: Option<String>,
    }
}

carrier_type! {
    pub struct AtprotoSnapshot {
        #[serde(default)]
        pub account: Option<AtprotoSiteAccount>,
        #[serde(default)]
        pub followers: CarrierPage<AtprotoFollower>,
        #[serde(default)]
        pub posts: BTreeMap<String, AtprotoPost>,
    }
}

carrier_type! {
    pub struct AtprotoAccount {
        pub did: String,
        pub handle: Option<String>,
        pub name: Option<String>,
        pub url: String,
        pub avatar: Option<String>,
    }
}

carrier_type! {
    pub struct AtprotoSiteAccount {
        pub did: String,
        pub handle: String,
        pub url: String,
        pub followers: u64,
        pub following: u64,
        pub posts: u64,
    }
}

carrier_type! {
    pub struct AtprotoFollower {
        pub account: AtprotoAccount,
        pub followed_at: String,
    }
}

carrier_type! {
    pub struct AtprotoReaction {
        pub account: AtprotoAccount,
        pub at: String,
    }
}

carrier_type! {
    pub struct AtprotoReply {
        pub uri: String,
        pub url: String,
        pub in_reply_to: String,
        pub account: AtprotoAccount,
        pub text: String,
        pub published: String,
    }
}

carrier_type! {
    pub struct AtprotoPost {
        pub counts: AtprotoCounts,
        pub likes: CarrierPage<AtprotoReaction>,
        pub reposts: CarrierPage<AtprotoReaction>,
        pub replies: CarrierPage<AtprotoReply>,
    }
}

carrier_type! {
    pub struct AtprotoCounts {
        pub likes: u64,
        pub reposts: u64,
        pub replies: u64,
    }
}

carrier_type! {
    pub struct WebmentionsSnapshot {
        #[serde(default)]
        pub pages: BTreeMap<String, WebmentionPage>,
    }
}

carrier_type! {
    pub struct WebmentionPage {
        pub counts: WebmentionCounts,
        pub list: CarrierPage<Webmention>,
    }
}

carrier_type! {
    pub struct WebmentionCounts {
        pub replies: u64,
        pub likes: u64,
        pub reposts: u64,
        pub bookmarks: u64,
        pub mentions: u64,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebmentionKind {
    Reply,
    Like,
    Repost,
    Bookmark,
    #[default]
    Mention,
}

carrier_type! {
    pub struct WebmentionAuthor {
        pub name: Option<String>,
        pub url: Option<String>,
        pub photo: Option<String>,
    }
}

carrier_type! {
    pub struct Webmention {
        pub source: String,
        pub url: String,
        pub kind: WebmentionKind,
        pub author: Option<WebmentionAuthor>,
        pub name: Option<String>,
        pub content: Option<String>,
        pub published: Option<String>,
        pub received: String,
        pub updated: Option<String>,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    #[default]
    ActivityPub,
    AtProto,
    Webmention,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReactionKind {
    #[default]
    Like,
    Repost,
}

carrier_type! {
    pub struct PostInteractions {
        pub counts: InteractionTotals,
        pub replies: CarrierPage<InteractionReply>,
        pub reactions: CarrierPage<InteractionReaction>,
    }
}

carrier_type! {
    pub struct InteractionTotals {
        pub likes: u64,
        pub reposts: u64,
        pub replies: u64,
        pub mentions: u64,
        pub bookmarks: u64,
    }
}

carrier_type! {
    pub struct InteractionAuthor {
        pub name: Option<String>,
        pub handle: Option<String>,
        pub url: Option<String>,
        pub avatar: Option<String>,
    }
}

carrier_type! {
    pub struct InteractionReply {
        pub network: Network,
        pub id: String,
        pub url: Option<String>,
        pub in_reply_to: Option<String>,
        pub author: InteractionAuthor,
        pub content: String,
        pub summary: Option<String>,
        pub sensitive: bool,
        pub attachments: Vec<ReplyAttachment>,
        pub published: String,
        pub updated: Option<String>,
    }
}

carrier_type! {
    pub struct InteractionReaction {
        pub network: Network,
        pub kind: ReactionKind,
        pub author: InteractionAuthor,
        pub at: String,
    }
}

impl SocialSnapshot {
    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        serde_json::from_str(json)
    }
}

fn liquid_value(value: &impl Serialize) -> Value {
    liquid::model::to_value(value).expect("social snapshots are plain data")
}

fn liquid_object(value: &impl Serialize) -> liquid::Object {
    match liquid_value(value) {
        Value::Object(object) => object,
        _ => liquid::Object::new(),
    }
}

/// An object in the `archival` global. Keys resolve to a child view, then to a
/// plain value, then to `fallback`, which is how an unknown path reads as the
/// carrier's empty answer instead of nil.
#[derive(Debug, Default)]
pub(crate) struct SocialView {
    children: Vec<(&'static str, SocialView)>,
    fields: liquid::Object,
    fallback: Option<Value>,
}

impl SocialView {
    pub(crate) fn new(snapshot: Option<&SocialSnapshot>) -> Self {
        let empty = SocialSnapshot::default();
        let snapshot = snapshot.unwrap_or(&empty);
        let keyed = |entries: liquid::Object, empty: Value| SocialView {
            fields: entries,
            fallback: Some(empty),
            ..Default::default()
        };
        let network = |mut fields: liquid::Object, key: &'static str, empty: Value| {
            let entries = match fields.remove(key) {
                Some(Value::Object(entries)) => entries,
                _ => liquid::Object::new(),
            };
            SocialView {
                children: vec![(key, keyed(entries, empty))],
                fields,
                fallback: None,
            }
        };
        let mut fields = liquid::Object::new();
        fields.insert("generated_at".into(), liquid_value(&snapshot.generated_at));
        SocialView {
            children: vec![
                (
                    "activitypub",
                    network(
                        liquid_object(&snapshot.activitypub),
                        "posts",
                        liquid_value(&ActivityPubPost::default()),
                    ),
                ),
                (
                    "atproto",
                    network(
                        liquid_object(&snapshot.atproto),
                        "posts",
                        liquid_value(&AtprotoPost::default()),
                    ),
                ),
                (
                    "webmentions",
                    network(
                        liquid_object(&snapshot.webmentions),
                        "pages",
                        liquid_value(&WebmentionPage::default()),
                    ),
                ),
                (
                    "interactions",
                    keyed(
                        liquid_object(&snapshot.interactions),
                        liquid_value(&PostInteractions::default()),
                    ),
                ),
            ],
            fields,
            fallback: None,
        }
    }

    fn child(&self, index: &str) -> Option<&SocialView> {
        self.children
            .iter()
            .find(|(key, _)| *key == index)
            .map(|(_, view)| view)
    }

    fn materialize(&self) -> liquid::Object {
        let mut object = self.fields.clone();
        for (key, child) in &self.children {
            object.insert((*key).into(), Value::Object(child.materialize()));
        }
        object
    }
}

impl fmt::Display for SocialView {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for (k, v) in ObjectView::iter(self) {
            write!(f, "{}: {} ", k, v.render())?;
        }
        Ok(())
    }
}

impl ValueView for SocialView {
    fn as_debug(&self) -> &dyn fmt::Debug {
        self
    }
    fn render(&self) -> liquid::model::DisplayCow<'_> {
        liquid::model::DisplayCow::Owned(Box::new(self))
    }
    fn source(&self) -> liquid::model::DisplayCow<'_> {
        liquid::model::DisplayCow::Owned(Box::new(self))
    }
    fn type_name(&self) -> &'static str {
        "object"
    }
    fn query_state(&self, state: liquid::model::State) -> bool {
        match state {
            liquid::model::State::Truthy => true,
            liquid::model::State::DefaultValue
            | liquid::model::State::Empty
            | liquid::model::State::Blank => ObjectView::size(self) == 0,
        }
    }
    fn to_kstr(&self) -> KStringCow<'_> {
        KStringCow::from_string(self.to_string())
    }
    fn to_value(&self) -> Value {
        Value::Object(self.materialize())
    }
    fn as_object(&self) -> Option<&dyn ObjectView> {
        Some(self)
    }
}

impl ObjectView for SocialView {
    fn as_value(&self) -> &dyn ValueView {
        self
    }
    fn size(&self) -> i64 {
        (self.children.len() + self.fields.len()) as i64
    }
    fn keys<'k>(&'k self) -> Box<dyn Iterator<Item = KStringCow<'k>> + 'k> {
        Box::new(
            self.children
                .iter()
                .map(|(key, _)| KStringCow::from_static(key))
                .chain(self.fields.keys().map(|k| k.as_ref().into())),
        )
    }
    fn values<'k>(&'k self) -> Box<dyn Iterator<Item = &'k dyn ValueView> + 'k> {
        Box::new(ObjectView::iter(self).map(|(_, v)| v))
    }
    fn iter<'k>(&'k self) -> Box<dyn Iterator<Item = (KStringCow<'k>, &'k dyn ValueView)> + 'k> {
        Box::new(
            self.children
                .iter()
                .map(|(key, view)| (KStringCow::from_static(key), view as &dyn ValueView))
                .chain(
                    self.fields
                        .iter()
                        .map(|(k, v)| (k.as_ref().into(), v.as_view())),
                ),
        )
    }
    fn contains_key(&self, index: &str) -> bool {
        self.child(index).is_some() || self.fields.contains_key(index)
    }
    fn get<'s>(&'s self, index: &str) -> Option<&'s dyn ValueView> {
        if let Some(child) = self.child(index) {
            return Some(child);
        }
        self.fields
            .get(index)
            .or(self.fallback.as_ref())
            .map(|v| v.as_view())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        constants::{MANIFEST_FILE_NAME, OBJECT_DEFINITION_FILE_NAME},
        site::Site,
        BuildOptions, FileSystemAPI, MemoryFileSystem, ObjectDefinition,
    };
    use std::path::{Path, PathBuf};

    const FULL: &str = include_str!("../tests/schemas/positive/archival_social/full.json");

    fn render(template: &str, social: Option<SocialSnapshot>) -> String {
        let mut fs = MemoryFileSystem::default();
        fs.write_str(Path::new(MANIFEST_FILE_NAME), String::new())
            .unwrap();
        fs.write_str(
            Path::new(OBJECT_DEFINITION_FILE_NAME),
            "[post]\nname = \"string\"\ntemplate = \"post\"\n".to_string(),
        )
        .unwrap();
        fs.write_str(
            Path::new("objects/post/hello.toml"),
            "name = \"Hello\"\n".to_string(),
        )
        .unwrap();
        fs.write_str(Path::new("pages/post.liquid"), template.to_string())
            .unwrap();
        fs.write_str(
            Path::new("pages/_card.liquid"),
            "{{ archival.interactions[post.path].counts.likes }}".to_string(),
        )
        .unwrap();
        let site = Site::load(&fs, Some("test")).unwrap();
        site.set_social(social);
        site.build(&mut fs, BuildOptions::default()).unwrap();
        fs.read_to_string(site.manifest.build_dir.join("post/hello.html"))
            .unwrap()
            .unwrap()
            .trim()
            .to_string()
    }

    fn full() -> Option<SocialSnapshot> {
        Some(SocialSnapshot::from_json(FULL).unwrap())
    }

    #[test]
    fn each_network_renders_its_carrier_answer_in_snake_case() {
        let rendered = render(
            "{{ archival.activitypub.posts[post.path].counts.likes }}|\
             {{ archival.activitypub.posts[post.path].replies.items[0].in_reply_to }}|\
             {{ archival.activitypub.followers.next }}|\
             {{ archival.activitypub.account.handle }}|\
             {{ archival.atproto.posts[post.path].replies.items[0].account.handle }}|\
             {{ archival.webmentions.pages[\"\"].list.items[0].author.name }}|\
             {{ archival.generated_at }}",
            full(),
        );
        assert_eq!(
            rendered,
            "4|https://example.com/_archival/ap/news/objects/post/hello|1761955200000.7|\
             @news@example.com|bob.example|Carol|2026-11-11T12:00:00Z"
        );
    }

    #[test]
    fn interactions_merge_every_network() {
        let rendered = render(
            "{{ archival.interactions[post.path].counts.likes }}|\
             {% for reply in archival.interactions[post.path].replies.items %}\
             {{ reply.network }}:{{ reply.author.handle }};{% endfor %}|\
             {{ archival.interactions[post.path].reactions.next }}",
            full(),
        );
        assert_eq!(
            rendered,
            "6|activitypub:@alice@social.example;atproto:bob.example;|eyJhcCI6IjE3NjIwNDE2MDAwMDAuMyJ9"
        );
    }

    #[test]
    fn a_path_nothing_reached_reads_as_the_empty_answer() {
        let template = "{{ archival.interactions[post.path].counts.likes }}|\
             {{ archival.activitypub.posts[post.path].replies.total }}|\
             {{ archival.activitypub.posts[post.path].replies.items.size }}|\
             {{ archival.atproto.posts[post.path].counts.reposts }}|\
             {{ archival.webmentions.pages[\"about\"].counts.mentions }}|\
             {{ archival.activitypub.posts[post.path].replies.next | default: \"end\" }}|\
             {{ archival.activitypub.account | default: \"none\" }}";
        assert_eq!(render(template, None), "0|0|0|0|0|end|none");
        let mut other = full().unwrap();
        let hello = other.interactions.remove("post/hello").unwrap();
        other.interactions.insert("post/other".to_string(), hello);
        assert_eq!(
            render(
                "{{ archival.interactions[post.path].counts.likes }}",
                Some(other)
            ),
            "0"
        );
    }

    #[test]
    fn partials_read_the_archival_global() {
        assert_eq!(render("{% render \"card\", post: post %}", full()), "6");
        assert_eq!(render("{% include \"card\" %}", full()), "6");
    }

    #[test]
    fn the_snapshot_parser_agrees_with_the_schema_fixtures() {
        let fixtures = |kind: &str| -> Vec<PathBuf> {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/schemas")
                .join(kind)
                .join("archival_social");
            std::fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect()
        };
        for path in fixtures("positive") {
            let json = std::fs::read_to_string(&path).unwrap();
            if let Err(e) = SocialSnapshot::from_json(&json) {
                panic!("{} should parse: {e}", path.display());
            }
        }
        for path in fixtures("negative") {
            let json = std::fs::read_to_string(&path).unwrap();
            assert!(
                SocialSnapshot::from_json(&json).is_err(),
                "{} should not parse",
                path.display()
            );
        }
    }

    #[test]
    fn an_object_cannot_shadow_the_archival_global() {
        for name in ["archival", "page"] {
            let definitions = format!("[{name}]\ntitle = \"string\"\n");
            assert!(
                ObjectDefinition::from_source(&definitions, &Default::default()).is_err(),
                "an object named {name} should be refused"
            );
        }
        let child = "[post]\ntitle = \"string\"\n[post.archival]\nname = \"string\"\n";
        assert!(ObjectDefinition::from_source(child, &Default::default()).is_ok());
        let field = "[post]\narchival = \"string\"\n";
        assert!(ObjectDefinition::from_source(field, &Default::default()).is_ok());
    }
}
