use super::BinaryCommand;
use crate::{
    binary::{
        command::{add_args, command_root, CommandConfig},
        ArchivalConfig, ExitStatus,
    },
    constants::API_URL,
    events::{ArchivalEvent, EditFieldEvent},
    fields::{FieldType, File},
    file_system_stdlib,
    object::ValuePath,
    object_definition::ObjectDefinition,
    sha_for_data, Archival, FieldValue,
};
use anyhow::Result;
use clap::{arg, value_parser, ArgMatches};
use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use reqwest::header::{HeaderMap, AUTHORIZATION};
use serde_json::json;
use std::{
    fs, io,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
};
use thiserror::Error;

const USAGE_HINT: &str =
    "usage: archival upload <object_type>/<object_name> <field> <file> (root objects take just <object_name>)";

#[derive(Error, Debug, Clone)]
pub enum UploadError {
    #[error("no access token found. Run archival login first.")]
    NotLoggedIn,
    #[error("file '{0}' doesn't exist")]
    FileNotExists(PathBuf),
    #[error("'{0}' is a directory - upload takes a single file")]
    FileIsDirectory(PathBuf),
    #[error("could not read file '{0}': {1}")]
    FileNotReadable(PathBuf, String),
    #[error("invalid object path '{0}'\n{USAGE_HINT}")]
    InvalidObjectPath(PathBuf),
    #[error("no object {0} found in this site.\n{1}\n{USAGE_HINT}")]
    ObjectNotFound(String, String),
    #[error("invalid object type '{0}'")]
    InvalidObjectType(String),
    #[error("'{0}' is not a field of object type '{1}'.\n{2}")]
    InvalidField(String, String, String),
    #[error("field '{0}' is a {1}, which cannot hold an upload.\n{2}")]
    NonUploadableType(String, String, String),
    #[error("upload failed: {0} responded {1}")]
    UploadFailed(String, String),
    #[error("could not infer repo from remotes: git command failed: {0}")]
    NoGit(String),
    #[error("could not infer repo from remotes: {0} - only github URLs supported.")]
    InferringRepoFailed(String),
}

impl FieldType {
    pub fn is_uploadable(&self) -> bool {
        matches!(
            self,
            FieldType::Audio | FieldType::Video | FieldType::Upload | FieldType::Image
        )
    }
}

fn response_error(r: reqwest::blocking::Response) -> String {
    let status = r.status();
    match r.text() {
        Ok(body) if !body.trim().is_empty() => format!("{} ({})", status, body.trim()),
        _ => status.to_string(),
    }
}

/// Endpoints from the api contain a raw filename, which needs escaping before
/// it can go back over the wire.
fn encode_path(path: &str) -> String {
    path.split('/')
        .map(|part| urlencoding::encode(part).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn uploadable_fields_hint(def: &ObjectDefinition) -> String {
    let uploadable: Vec<_> = def
        .fields
        .iter()
        .filter(|(_, f)| f.r#type.is_uploadable())
        .map(|(name, f)| format!("{} ({})", name, f.r#type))
        .collect();
    if uploadable.is_empty() {
        format!("'{}' has no uploadable fields", def.name)
    } else {
        format!("uploadable fields: {}", uploadable.join(", "))
    }
}

pub struct Command {}
impl BinaryCommand for Command {
    fn name(&self) -> &str {
        "upload"
    }
    fn cli(&self, cmd: clap::Command) -> clap::Command {
        add_args(cmd.about("uploads a local file to archival")
            .arg(
                arg!([object] "The object to upload data for")
                    .required(true)
                    .value_parser(value_parser!(PathBuf)),
            )
            .arg(
                arg!([field] "The field to upload to")
                    .required(true)
                    .value_parser(value_parser!(String)),
            )
            .arg(
                arg!(-r --repo <repo_name> "A repo name (e.g. github/your-org/your-site) to use for this upload. If not provided, will be inferred from the first git remote.")
                    .value_parser(value_parser!(String)),
            )
            .arg(
                arg!([file] "The file to upload")
                    .required(true)
                    .value_parser(value_parser!(PathBuf)),
            ), CommandConfig::archival_site())
    }
    fn handler(
        &self,
        args: &ArgMatches,
        _quit: Arc<AtomicBool>,
    ) -> Result<crate::binary::ExitStatus> {
        let root_dir = command_root(args);
        // Fail fast if we aren't logged in
        let config = ArchivalConfig::get();
        let access_token = config.access_token.ok_or(UploadError::NotLoggedIn)?;
        // Fail fast if file doesn't exist
        let file_path = args.get_one::<PathBuf>("file").unwrap();
        match fs::metadata(file_path) {
            Ok(meta) if meta.is_dir() => {
                return Err(UploadError::FileIsDirectory(file_path.to_owned()).into())
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(UploadError::FileNotExists(file_path.to_owned()).into())
            }
            Err(e) => {
                return Err(
                    UploadError::FileNotReadable(file_path.to_owned(), e.to_string()).into(),
                )
            }
        }
        let object = args.get_one::<PathBuf>("object").unwrap();
        let object_name = object
            .with_extension("")
            .file_name()
            .ok_or_else(|| UploadError::InvalidObjectPath(object.to_owned()))?
            .to_string_lossy()
            .to_string();
        let mut object_type = object
            .parent()
            .ok_or_else(|| UploadError::InvalidObjectPath(object.to_owned()))?
            .to_string_lossy()
            .to_string();
        // If we don't find the object type, it's likely that we mean to use a
        // root object, in which case they're the same. A check below will make
        // sure that this is truly the case (object_exists)
        let is_root_object = object_type.is_empty();
        if is_root_object {
            object_name.clone_into(&mut object_type);
        }
        let field = args.get_one::<String>("field").unwrap();
        let field_path = ValuePath::from_string(field);
        // Set up an archival site to make sure we're able to modify fields
        let fs = file_system_stdlib::NativeFileSystem::new(&root_dir);
        let archival = if let Some(upload_prefix) =
            args.get_one::<String>("upload-prefix").map(|s| s.as_str())
        {
            Archival::new_with_upload_prefix(fs, upload_prefix)?
        } else {
            Archival::new(fs)?
        };
        // Make sure that the specified object exists. An unknown object type
        // surfaces here as an Err rather than Ok(false).
        if !matches!(archival.object_exists(&object_type, &object_name), Ok(true)) {
            let known_types = archival
                .site
                .object_definitions
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let descriptor = if is_root_object {
                format!("'{}'", object_name)
            } else {
                format!("'{}' of type '{}'", object_name, object_type)
            };
            return Err(UploadError::ObjectNotFound(
                descriptor,
                format!("known object types: {}", known_types),
            )
            .into());
        }
        // Find the specified object definition
        let obj_def = archival
            .site
            .object_definitions
            .get(&object_type)
            .ok_or_else(|| UploadError::InvalidObjectType(object_type.to_owned()))?;
        // Find the specified field in the object definition
        let field_def = field_path.get_field_definition(obj_def).map_err(|_| {
            UploadError::InvalidField(
                field.to_owned(),
                object_type.to_owned(),
                uploadable_fields_hint(obj_def),
            )
        })?;
        // Validate that this is an ok field type to upload
        if !field_def.is_uploadable() {
            return Err(UploadError::NonUploadableType(
                field.to_owned(),
                field_def.to_string(),
                uploadable_fields_hint(obj_def),
            )
            .into());
        }
        // Validate repo
        let repo_id = if let Some(repo) = args.get_one::<String>("repo") {
            repo.to_string()
        } else {
            let github_remote_match = Regex::new(r"github.com.+?\b(.+)\/(.+)\.git").unwrap();
            let git_command = std::process::Command::new("git")
                .current_dir(root_dir)
                .arg("remote")
                .arg("-v")
                .output()
                .map_err(|e| UploadError::NoGit(e.to_string()))?;
            let output = String::from_utf8(git_command.stdout.as_slice().to_vec())
                .map_err(|err| UploadError::InferringRepoFailed(err.to_string()))?;
            let first_origin = output.split("\n").next().ok_or_else(|| {
                UploadError::InferringRepoFailed(format!("No origins found in {}", output))
            })?;
            let first_match = github_remote_match
                .captures_iter(first_origin)
                .next()
                .ok_or_else(|| {
                    UploadError::InferringRepoFailed(format!(
                        "No github origin found in {}",
                        first_origin
                    ))
                })?;
            format!(
                "github/{}/{}",
                first_match.get(1).unwrap().as_str(),
                first_match.get(2).unwrap().as_str()
            )
        };
        // Ok, this looks legit. Upload the file. Uploads usually live outside
        // of the site root, so read them from the OS, not the site's fs.
        let file_data = fs::read(file_path)
            .map_err(|e| UploadError::FileNotReadable(file_path.to_owned(), e.to_string()))?;
        let sha = sha_for_data(&file_data);
        let mime = mime_guess::from_path(file_path);
        let mut file = File::from_mime_guess(mime);
        file.sha = sha;
        file.filename = file_path
            .file_name()
            .map_or("".to_string(), |f| f.to_string_lossy().to_string());
        let bar = ProgressBar::new_spinner();
        bar.set_style(ProgressStyle::with_template("{msg} {spinner}").unwrap());
        bar.set_message(format!("uploading {}", file_path.to_string_lossy()));
        let mut headers = HeaderMap::new();
        headers.append(
            AUTHORIZATION,
            format!("Bearer {}", access_token).parse().unwrap(),
        );
        let client = reqwest::blocking::Client::builder()
            .default_headers(headers)
            .build()?;
        // The upload endpoint is keyed by the repo's durable object id, which
        // only the api can resolve, so ask it where to send this file.
        let create_url = format!("{}/create-upload/{}", API_URL, repo_id);
        let r = client.post(&create_url).json(&file).send()?;
        let create_status = r.status();
        if !create_status.is_success() {
            return Err(UploadError::UploadFailed(create_url, response_error(r)).into());
        }
        match r.json::<api_response::CreateUploadResponse>()? {
            api_response::CreateUploadResponse::Existed => {
                bar.finish_with_message(format!("{} was already uploaded", file.filename));
            }
            api_response::CreateUploadResponse::Created(upload) => {
                let upload_url = format!("{}/{}", API_URL, encode_path(&upload.endpoint));
                // TODO: set a max chunk size and parallelize chunked uploads
                let put_r = client
                    .put(&upload_url)
                    .query(&[
                        ("uploadId", &upload.upload_id),
                        ("partNumber", &"1".to_string()),
                    ])
                    .body(file_data)
                    .send()?;
                if !put_r.status().is_success() {
                    return Err(UploadError::UploadFailed(upload_url, response_error(put_r)).into());
                }
                let part = put_r.json::<api_response::UploadedPart>()?;
                let r = client
                    .post(&upload_url)
                    .query(&[("action", "mpu-complete"), ("uploadId", &upload.upload_id)])
                    .body(json!({ "parts": vec![part] }).to_string())
                    .send()?;
                if !r.status().is_success() {
                    return Err(UploadError::UploadFailed(upload_url, response_error(r)).into());
                }
                bar.finish();
            }
        }
        let field_data = FieldValue::File(file);
        // Now write our file
        archival.send_event(
            ArchivalEvent::EditField(EditFieldEvent {
                object: object_type.clone(),
                filename: object_name.clone(),
                path: ValuePath::empty(),
                value: Some(field_data.clone()),
                field: field.to_string(),
                source: None,
            }),
            None,
        )?;
        if let FieldValue::File(fd) = field_data {
            println!(
                "Wrote {} {} {}: {:?}",
                object_type, object_name, field_path, fd
            );
        } else {
            panic!("Invalid field data");
        }
        Ok(ExitStatus::Ok)
    }
}

mod api_response {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Deserialize)]
    pub enum CreateUploadResponse {
        Existed,
        Created(NewUpload),
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct NewUpload {
        pub upload_id: String,
        pub endpoint: String,
    }

    #[derive(Debug, Deserialize, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct UploadedPart {
        pub part_number: usize,
        pub etag: String,
    }
}
