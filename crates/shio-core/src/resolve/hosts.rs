use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use reqwest::header;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

use super::{Resolved, Target, read_bounded};
use crate::error::{Result, ShioError};

const GOFILE_API: &str = "https://api.gofile.io";
// gofile's anti-bot salt from its `wt.obf.js`; gofile links fail until this is updated.
const GOFILE_SALT: &str = "12af056dacea0b";
const GOFILE_TOKEN_WINDOW_SECS: u64 = 14_400;
const MAX_FOLDER_FILES: usize = 256;
const MAX_FOLDER_NAME_BYTES: usize = 240;

static TOKEN: Mutex<Option<String>> = Mutex::new(None);

#[derive(Debug, PartialEq, Eq)]
enum Host {
    Pixeldrain(String),
    Gofile(String),
}

pub(super) async fn known(client: &reqwest::Client, url: &Url) -> Option<Result<Target>> {
    Some(match classify(url)? {
        Host::Pixeldrain(url) => Ok(Target::File(Resolved {
            url,
            headers: Vec::new(),
        })),
        Host::Gofile(id) => gofile(client, url, &id).await,
    })
}

fn classify(url: &Url) -> Option<Host> {
    let host = url.host_str()?;
    let segments: Vec<&str> = url.path_segments()?.collect();
    match (host, segments.as_slice()) {
        (
            "pixeldrain.com" | "www.pixeldrain.com" | "pixeldrain.net" | "www.pixeldrain.net",
            ["u", id],
        ) if is_id(id, 32, false) => Some(Host::Pixeldrain(format!(
            "https://{host}/api/file/{id}?download"
        ))),
        ("gofile.io" | "www.gofile.io", ["d", id]) if is_id(id, 64, true) => {
            Some(Host::Gofile((*id).to_owned()))
        },
        _ => None,
    }
}

fn is_id(id: &str, max_len: usize, dashes: bool) -> bool {
    (4..=max_len).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || (dashes && b == b'-'))
}

#[derive(Deserialize)]
struct Reply {
    status: String,
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Deserialize)]
struct Account {
    token: String,
}

#[derive(Deserialize)]
struct Content {
    #[serde(rename = "type")]
    kind: String,
    name: String,
    link: Option<String>,
    #[serde(default)]
    children: BTreeMap<String, Self>,
}

#[derive(Debug, PartialEq, Eq)]
enum Pick {
    File(String),
    Folder(Vec<String>),
}

fn refused() -> ShioError {
    ShioError::Other("gofile refused the request".into())
}

fn unreadable(error: ShioError) -> ShioError {
    if crate::probe::is_transient(&error) {
        error
    } else {
        refused()
    }
}

async fn gofile(client: &reqwest::Client, url: &Url, id: &str) -> Result<Target> {
    let selector = url
        .query_pairs()
        .find(|(key, _)| key == "file")
        .map(|(_, value)| value.into_owned())
        .or_else(|| crate::filename::fragment_name(url));
    for _ in 0..2 {
        let token = token(client).await?;
        let Some(content) = contents(client, id, &token).await? else {
            *TOKEN.lock() = None;
            continue;
        };
        return match pick(content, selector.as_deref())? {
            Pick::File(link) => {
                crate::engine::validate_http_url(&link).map_err(|_| ShioError::NoFileOnPage)?;
                Ok(Target::File(Resolved {
                    url: link,
                    headers: vec![("Cookie".into(), format!("accountToken={token}"))],
                }))
            },
            Pick::Folder(names) => Ok(Target::Folder(names)),
        };
    }
    Err(refused())
}

async fn token(client: &reqwest::Client) -> Result<String> {
    let cached = TOKEN.lock().clone();
    if let Some(token) = cached {
        return Ok(token);
    }
    let response = client.post(format!("{GOFILE_API}/accounts")).send().await?;
    let error = crate::probe::http_error(&response);
    let Ok(reply) = serde_json::from_str::<Reply>(&read_bounded(response).await?) else {
        return Err(unreadable(error));
    };
    if reply.status != "ok" {
        return Err(refused());
    }
    let account: Account = serde_json::from_value(reply.data).map_err(|_| refused())?;
    *TOKEN.lock() = Some(account.token.clone());
    Ok(account.token)
}

// `None` means the guest token was rejected and should be replaced.
async fn contents(client: &reqwest::Client, id: &str, token: &str) -> Result<Option<Content>> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let response = client
        .get(format!("{GOFILE_API}/contents/{id}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(
            "X-Website-Token",
            website_token(crate::worker::USER_AGENT, token, now),
        )
        .header("X-BL", "en-US")
        .send()
        .await?;
    let status = response.status();
    let error = crate::probe::http_error(&response);
    let Ok(reply) = serde_json::from_str::<Reply>(&read_bounded(response).await?) else {
        return if status.as_u16() == 401 {
            Ok(None)
        } else {
            Err(unreadable(error))
        };
    };
    match reply.status.as_str() {
        "ok" => serde_json::from_value(reply.data)
            .map(Some)
            .map_err(|_| refused()),
        "error-notFound" => Err(ShioError::Http {
            code: 404,
            retry_after: None,
        }),
        "error-passwordRequired" => Err(ShioError::PagePasswordProtected),
        "error-token" | "error-notPremium" => Ok(None),
        _ => Err(refused()),
    }
}

fn website_token(user_agent: &str, token: &str, unix_secs: u64) -> String {
    let window = unix_secs / GOFILE_TOKEN_WINDOW_SECS;
    let digest = Sha256::digest(format!(
        "{user_agent}::en-US::{token}::{window}::{GOFILE_SALT}"
    ));
    digest.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

fn pick(content: Content, selector: Option<&str>) -> Result<Pick> {
    let mut files: Vec<Content> = if content.kind == "file" {
        vec![content]
    } else {
        content
            .children
            .into_values()
            .filter(|child| child.kind == "file")
            .collect()
    };
    files.sort_by(|left, right| left.name.cmp(&right.name));
    let chosen = match selector {
        Some(selector) => files.into_iter().find(|file| {
            file.name == selector || crate::filename::sanitize_filename(&file.name) == selector
        }),
        None if files.len() > 1 => {
            let names: Vec<String> = files
                .into_iter()
                .map(|file| file.name)
                .filter(|name| name.len() <= MAX_FOLDER_NAME_BYTES)
                .take(MAX_FOLDER_FILES)
                .collect();
            return if names.is_empty() {
                Err(ShioError::NoFileOnPage)
            } else {
                Ok(Pick::Folder(names))
            };
        },
        None => files.into_iter().next(),
    };
    chosen
        .and_then(|file| file.link)
        .map(Pick::File)
        .ok_or(ShioError::NoFileOnPage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_hosts_are_matched_by_exact_paths() {
        for (url, expected) in [
            (
                "https://pixeldrain.com/u/AbCd1234",
                Some(Host::Pixeldrain(
                    "https://pixeldrain.com/api/file/AbCd1234?download".into(),
                )),
            ),
            (
                "https://pixeldrain.net/u/AbCd1234",
                Some(Host::Pixeldrain(
                    "https://pixeldrain.net/api/file/AbCd1234?download".into(),
                )),
            ),
            ("https://pixeldrain.com/l/AbCd1234", None),
            ("https://pixeldrain.com/u/AbCd1234/extra", None),
            ("https://pixeldrain.com/u/Ab%2F..%2Fx", None),
            (
                "https://gofile.io/d/Xy12-ab",
                Some(Host::Gofile("Xy12-ab".into())),
            ),
            ("https://gofile.io/", None),
            ("https://evil.example/u/AbCd1234", None),
        ] {
            assert_eq!(classify(&Url::parse(url).unwrap()), expected, "{url}");
        }
    }

    #[test]
    fn gofile_website_token_matches_the_web_client() {
        assert_eq!(
            website_token("Shio/0.2.0", "abc", 1_771_200_000),
            "b363e42f652e04831ca8fa1d84edeaf5c04a27115f39b79e3458f1a3e8994108"
        );
    }

    #[test]
    fn gofile_picks_by_fragment_or_lists_folders() {
        let file = |name: &str| Content {
            kind: "file".into(),
            name: name.into(),
            link: Some(format!("https://cdn.example/{name}")),
            children: BTreeMap::new(),
        };
        let folder = |names: &[&str]| Content {
            kind: "folder".into(),
            name: "root".into(),
            link: None,
            children: names
                .iter()
                .enumerate()
                .map(|(index, name)| (index.to_string(), file(name)))
                .collect(),
        };
        assert_eq!(
            pick(folder(&["b.zip", "a.mkv"]), Some("b.zip")).unwrap(),
            Pick::File("https://cdn.example/b.zip".into())
        );
        assert_eq!(
            pick(folder(&["a.mkv"]), None).unwrap(),
            Pick::File("https://cdn.example/a.mkv".into())
        );
        assert_eq!(
            pick(folder(&["b.zip", "a.mkv"]), None).unwrap(),
            Pick::Folder(vec!["a.mkv".into(), "b.zip".into()])
        );
        assert!(matches!(
            pick(folder(&["a.mkv"]), Some("other.zip")),
            Err(ShioError::NoFileOnPage)
        ));
    }
}
