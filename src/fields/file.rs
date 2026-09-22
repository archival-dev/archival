use crate::liquid_kstring::KStringCow;
use crate::{fields::FieldConfig, object::Renderable};
use liquid::{model, ObjectView, ValueView};
use mime_guess::{mime::FromStrError, Mime, MimeGuess};
use ordermap::OrderMap;
use serde::{Deserialize, Serialize};
use std::{fmt::Display, str::FromStr};
use thiserror::Error;
use tracing::warn;

#[derive(Error, Debug)]
pub enum FileError {
    #[error("Missing field {0}")]
    MissingField(String),
}

#[derive(Debug, Default, Clone, PartialEq, PartialOrd, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub enum DisplayType {
    Image,
    Video,
    Audio,
    #[default]
    Download,
}
impl DisplayType {
    fn to_str(&self) -> &str {
        match self {
            DisplayType::Image => "image",
            DisplayType::Audio => "audio",
            DisplayType::Video => "video",
            DisplayType::Download => "upload",
        }
    }
}
impl<'a> From<&'a DisplayType> for &'a str {
    fn from(value: &'a DisplayType) -> Self {
        value.to_str()
    }
}
impl Display for DisplayType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.to_str())
    }
}
impl From<DisplayType> for String {
    fn from(value: DisplayType) -> Self {
        value.to_str().to_string()
    }
}
impl From<&str> for DisplayType {
    fn from(value: &str) -> Self {
        match value {
            "image" => DisplayType::Image,
            "audio" => DisplayType::Audio,
            "video" => DisplayType::Video,
            "upload" => DisplayType::Download,
            t => panic!("invalid file type {}", t),
        }
    }
}
impl ValueView for DisplayType {
    fn as_scalar(&self) -> Option<model::ScalarCow<'_>> {
        Some(self.to_kstr().into())
    }
    fn as_debug(&self) -> &dyn std::fmt::Debug {
        self
    }

    fn render(&self) -> liquid::model::DisplayCow<'_> {
        model::DisplayCow::Owned(Box::new(self))
    }

    fn source(&self) -> liquid::model::DisplayCow<'_> {
        model::DisplayCow::Owned(Box::new(self))
    }

    fn type_name(&self) -> &'static str {
        "DisplayType"
    }

    fn query_state(&self, _state: liquid::model::State) -> bool {
        false
    }

    fn to_kstr(&self) -> KStringCow<'_> {
        KStringCow::from(self.to_str())
    }

    fn to_value(&self) -> liquid_core::Value {
        self.as_scalar().to_value()
    }
}

#[derive(
    Debug, ObjectView, ValueView, Deserialize, Serialize, Clone, PartialEq, PartialOrd, Hash,
)]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub struct RenderedFile {
    pub display_type: DisplayType,
    pub filename: String,
    pub sha: String,
    pub mime: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub url: String,
}
impl RenderedFile {
    pub fn from_file(file: File, field_config: &FieldConfig) -> Self {
        let url = file.url(field_config);
        Self {
            display_type: file.display_type,
            filename: file.filename,
            sha: file.sha,
            mime: file.mime,
            name: file.name,
            description: file.description,
            url,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, PartialOrd, Hash)]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub struct File {
    pub display_type: DisplayType,
    pub filename: String,
    pub sha: String,
    pub mime: String,
    pub name: Option<String>,
    pub description: Option<String>,
}

impl File {
    pub fn new(
        sha: &str,
        name: Option<&str>,
        description: Option<&str>,
        filename: &str,
        mime: &str,
        display_type: DisplayType,
    ) -> Self {
        Self {
            // url: Self::_url(sha, filename),
            sha: sha.to_string(),
            name: name.map(|n| n.to_string()),
            description: description.map(|d| d.to_string()),
            filename: filename.to_string(),
            mime: mime.to_string(),
            display_type,
        }
    }
    pub fn from_mime(mime_str: &str) -> Result<Self, FromStrError> {
        let mime = Mime::from_str(mime_str)?;
        let mut f = match mime.type_() {
            mime_guess::mime::VIDEO => Self::video(),
            mime_guess::mime::AUDIO => Self::audio(),
            mime_guess::mime::IMAGE => Self::image(),
            _ => Self::download(),
        };
        f.mime = mime.to_string();
        Ok(f)
    }
    pub fn from_mime_guess(mime: MimeGuess) -> Self {
        let m_type = mime.first_or_octet_stream();
        let mut f = match m_type.type_() {
            mime_guess::mime::VIDEO => Self::video(),
            mime_guess::mime::AUDIO => Self::audio(),
            mime_guess::mime::IMAGE => Self::image(),
            _ => Self::download(),
        };
        f.mime = m_type.to_string();
        f
    }
    pub fn get_key(&self, str: &str) -> Option<&str> {
        match str {
            "sha" => Some(&self.sha),
            "name" => self.name.as_deref(),
            "description" => self.description.as_deref(),
            "filename" => Some(&self.filename),
            "mime" => Some(&self.mime),
            "display_type" => Some(self.display_type.to_str()),
            _ => None,
        }
    }
    pub fn fill_from_toml_map(
        mut self,
        map: &toml::map::Map<String, toml::Value>,
    ) -> Result<Self, FileError> {
        for (k, v) in map {
            self.fill_field(k, || {
                v.as_str()
                    .map(|v| v.to_string())
                    .ok_or_else(|| FileError::MissingField(k.into()))
            })?;
        }
        Ok(self)
    }
    pub fn fill_from_json_map(
        mut self,
        map: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Self, FileError> {
        for (k, v) in map {
            self.fill_field(k, || {
                v.as_str()
                    .map(|v| v.to_string())
                    .ok_or_else(|| FileError::MissingField(k.into()))
            })?;
        }
        Ok(self)
    }
    fn fill_field(
        &mut self,
        k: &String,
        get_val: impl Fn() -> Result<String, FileError>,
    ) -> Result<(), FileError> {
        match &k[..] {
            "sha" => {
                self.sha = get_val()?;
            }
            "name" => self.name = Some(get_val()?),
            "description" => self.description = Some(get_val()?),
            "filename" => {
                self.filename = get_val()?;
            }
            "mime" => self.mime = get_val()?,
            "display_type" => self.display_type = DisplayType::from(get_val()?.as_str()),
            _ => {
                warn!("unknown file field {}", k);
            }
        }
        Ok(())
    }
    pub fn to_toml(&self) -> toml::map::Map<std::string::String, toml::Value> {
        let mut m = toml::map::Map::new();
        for (k, v) in self.clone().into_map(None) {
            m.insert(k.to_string(), toml::Value::String(v));
        }
        m
    }
    pub fn image() -> Self {
        let mime = "image/*";
        Self {
            sha: "".to_string(),
            name: None,
            description: None,
            filename: "".to_string(),
            mime: mime.to_string(),
            display_type: DisplayType::Image,
        }
    }
    pub fn video() -> Self {
        let mime = "video/*";
        Self {
            sha: "".to_string(),
            name: None,
            description: None,
            filename: "".to_string(),
            mime: mime.to_string(),
            display_type: DisplayType::Video,
        }
    }
    pub fn audio() -> Self {
        let mime = "audio/*";
        Self {
            sha: "".to_string(),
            name: None,
            description: None,
            filename: "".to_string(),
            mime: mime.to_string(),
            display_type: DisplayType::Audio,
        }
    }
    pub fn download() -> Self {
        let mime = "*/*";
        Self {
            sha: "".to_string(),
            name: None,
            description: None,
            filename: "".to_string(),
            mime: mime.to_string(),
            display_type: DisplayType::Download,
        }
    }
    pub fn is_valid(&self) -> bool {
        self.sha.len() == 64 && self.sha.chars().all(|c| c.is_ascii_hexdigit())
    }
    pub fn url(&self, config: &FieldConfig) -> String {
        if self.sha.is_empty() {
            return "".to_string();
        }
        if self.filename.is_empty() {
            format!(
                "{}/{}{}",
                config.uploads_url, config.upload_prefix, self.sha
            )
        } else {
            format!(
                "{}/{}{}/{}",
                config.uploads_url,
                config.upload_prefix,
                self.sha,
                urlencoding::encode(&self.filename)
            )
        }
    }
    pub fn into_map(
        self,
        field_config_for_render: Option<&FieldConfig>,
    ) -> OrderMap<&'static str, String> {
        // NOTE: order matters here, and should match the layout above
        let url = field_config_for_render.map(|f| self.url(f));
        let mut m = OrderMap::new();
        m.insert("display_type", self.display_type.to_string());
        m.insert("filename", self.filename);
        m.insert("sha", self.sha);
        m.insert("mime", self.mime);
        if let Some(name) = self.name {
            m.insert("name", name);
        }
        if let Some(description) = self.description {
            m.insert("description", description);
        }
        if let Some(url) = url {
            m.insert("url", url);
        }
        m
    }
}

impl Renderable for File {
    type Output = RenderedFile;
    fn rendered(self, field_config: &FieldConfig) -> Self::Output {
        RenderedFile::from_file(self, field_config)
    }
}

#[cfg(feature = "json-schema")]
impl File {
    pub fn to_json_schema_property(
        description: &str,
        field_path: &crate::ValuePath,
        field_type: &crate::FieldType,
        display_type: DisplayType,
        options: &mut crate::json_schema::ObjectSchemaOptions,
    ) -> crate::json_schema::ObjectSchema {
        use serde_json::json;
        let mut property = serde_json::Map::new();
        property.insert("type".to_string(), "object".into());
        property.insert("description".to_string(), description.to_string().into());
        let mut properties = serde_json::Map::new();
        properties.insert(
            "sha".into(),
            json!({
                "type": "string",
                "description": "a string representing the hex-encoded sha256 hash content of this file",
                // Causes failures when submitting to openAI, so omit for now
                // "minLength": 64,
                // "maxLength": 64,
            }),
        );
        properties.insert(
            "name".into(),
            json!({
                "type": "string",
                "description": "the name of the file",
            }),
        );
        properties.insert(
            "description".into(),
            json!({
                "type": "string",
                "description": "a description of the file",
            }),
        );
        properties.insert(
            "filename".into(),
            json!({
                "type": "string",
                "description": "the filename of this file",
            }),
        );
        properties.insert(
            "mime".into(),
            json!({
                "type": "string",
                "description": "the mime type of this file",
            }),
        );
        properties.insert(
            "display_type".into(),
            json!({
                "const": display_type.to_str(),
                "type": "string",
                "description": "the display type of this file",
            }),
        );
        property.insert("properties".into(), properties.into());
        let mut required_fields = vec!["sha", "filename", "mime", "display_type"];
        if options.all_fields_required {
            required_fields.push("name");
            required_fields.push("description");
        }
        property.insert("required".into(), required_fields.into());
        property.insert("additionalProperties".into(), false.into());
        options.decorate(field_path, field_type, &mut property);
        property
    }
}

#[cfg(test)]
pub mod tests {

    use super::*;

    lazy_static::lazy_static! {
        static ref FC: FieldConfig = FieldConfig {
            uploads_url: "http://foo.com".to_string(),
            upload_prefix: "".to_string(),
        };
    }

    #[test]
    fn files_have_urls_when_specific() {
        let mut i = File::image();
        i.mime = "image/jpeg".to_string();
        let mut a = File::audio();
        a.mime = "audio/ogg".to_string();
        let mut d = File::download();
        d.mime = "application/pdf".to_string();
        let mut v = File::video();
        v.mime = "video/mp4".to_string();
        let one_of_each = [i, a, d, v];
        for mut file in one_of_each {
            file.sha = "fake-sha".to_string();
            println!("{}", file.url(&FC));
            assert!(!file.url(&FC).is_empty());
        }
    }
    #[test]
    fn files_urls_are_relative_to_uploads_url() {
        let mut file = File::image();
        file.filename = "image.png".to_string();
        file.sha = "fake-sha".to_string();
        println!("{}", file.url(&FC));
        assert_eq!(file.url(&FC), "http://foo.com/fake-sha/image.png");
    }
    #[test]
    fn files_urls_include_uploads_prefix() {
        let mut file = File::image();
        file.filename = "image.png".to_string();
        file.sha = "fake-sha".to_string();
        let fc = FieldConfig {
            uploads_url: "http://foo.com".to_string(),
            upload_prefix: "repo-doid/".to_string(),
        };
        println!("{}", file.url(&fc));
        assert_eq!(file.url(&fc), "http://foo.com/repo-doid/fake-sha/image.png");
    }
    #[test]
    fn file_validity() {
        let mut file = File::image();
        assert!(!file.is_valid());
        file.filename = "image.png".to_string();
        file.sha = "fake-sha".to_string();
        assert!(!file.is_valid());
        file.sha = "0a20136df0a8221878ed1109dfb0511dbb7ba3d4b87460963a86cb049434d38a".to_string();
        assert!(file.is_valid());
    }
}
