mod hosts;
mod page;

use std::cmp::Reverse;
use std::collections::HashSet;
use std::time::Duration;

use reqwest::header;
use url::Url;

use self::page::Page;
use crate::error::{Result, ShioError};
use crate::filename::{
    fragment_name, parse_content_disposition, path_filename, rar_part, sevenz_volume,
};

const MAX_REQUESTS: usize = 12;
const MAX_DEPTH: u8 = 6;
const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_STEPS_PER_PAGE: usize = 8;
const MIN_SCORE: i32 = 40;
const MAX_WAIT: Duration = Duration::from_secs(15);
const FORCED_SCORE: i32 = 1000;
const NAVIGATION_WORDS: [&str; 18] = [
    "login", "log in", "sign", "register", "premium", "upgrade", "buy", "report", "abuse", "faq",
    "terms", "privacy", "contact", "dmca", "upload", "preview", "share", "account",
];
const ASSET_EXTENSIONS: [&str; 15] = [
    "css", "js", "mjs", "png", "jpg", "jpeg", "gif", "svg", "ico", "webp", "woff", "woff2", "ttf",
    "eot", "map",
];
const FILE_EXTENSIONS: [&str; 34] = [
    "7z", "apk", "appimage", "avi", "bin", "bz2", "cab", "deb", "dmg", "epub", "exe", "flac", "gz",
    "img", "iso", "jar", "m4a", "mkv", "mov", "mp3", "mp4", "msi", "pdf", "pkg", "rar", "rpm",
    "tar", "tgz", "txz", "wav", "webm", "xz", "zip", "zst",
];
const PAGE_TYPES: [&str; 5] = [
    "text/html",
    "application/xhtml+xml",
    "application/json",
    "text/javascript",
    "application/javascript",
];
const SECOND_LEVEL_SUFFIXES: [&str; 10] = [
    "ac", "co", "com", "edu", "go", "gov", "ne", "net", "or", "org",
];

pub(crate) struct Resolved {
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
}

pub(crate) enum Target {
    File(Resolved),
    Folder(Vec<String>),
}

pub(crate) async fn resolve(client: &reqwest::Client, url: &str) -> Result<Target> {
    let url = Url::parse(url).map_err(|_| ShioError::NoFileOnPage)?;
    if let Some(result) = hosts::known(client, &url).await {
        return result;
    }
    let resolved = search(client, url).await?;
    crate::engine::validate_http_url(&resolved.url).map_err(|_| ShioError::NoFileOnPage)?;
    Ok(Target::File(resolved))
}

struct Step {
    score: i32,
    depth: u8,
    trusted: bool,
    markup_link: bool,
    request: Request,
}

#[derive(Clone)]
struct Request {
    url: Url,
    body: Option<String>,
    htmx: bool,
    referer: Option<Url>,
    wait: Duration,
}

impl Request {
    const fn get(url: Url, referer: Option<Url>) -> Self {
        Self {
            url,
            body: None,
            htmx: false,
            referer,
            wait: Duration::ZERO,
        }
    }
}

#[derive(Default)]
struct Queue {
    steps: Vec<(Step, Reverse<u64>)>,
    next: u64,
}

impl Queue {
    fn push(&mut self, step: Step, hint: &Hint) {
        if step.depth > MAX_DEPTH || !(step.trusted || hint.is_name(&step.request.url)) {
            return;
        }
        self.next += 1;
        self.steps.push((step, Reverse(self.next)));
    }

    fn pop(&mut self) -> Option<Step> {
        let best = self
            .steps
            .iter()
            .enumerate()
            .max_by_key(|(_, (step, seq))| (step.score, *seq))
            .map(|(index, _)| index)?;
        Some(self.steps.swap_remove(best).0)
    }
}

#[derive(Default)]
struct Evidence {
    captcha: bool,
    challenged: bool,
    password: bool,
}

impl Evidence {
    const fn into_error(self) -> ShioError {
        if self.password {
            ShioError::PagePasswordProtected
        } else if self.captcha {
            ShioError::PageNeedsCaptcha
        } else if self.challenged {
            ShioError::BrowserCheck
        } else {
            ShioError::NoFileOnPage
        }
    }
}

async fn search(client: &reqwest::Client, start: Url) -> Result<Resolved> {
    let mut hint = Hint::from_url(&start);
    let mut origins = vec![site(&start)];
    let mut queue = Queue::default();
    let mut visited = HashSet::new();
    let mut evidence = Evidence::default();
    let mut requests = 0;
    queue.push(
        Step {
            score: FORCED_SCORE,
            depth: 0,
            trusted: true,
            markup_link: false,
            request: Request::get(start, None),
        },
        &hint,
    );

    while requests < MAX_REQUESTS
        && let Some(step) = queue.pop()
    {
        let request = &step.request;
        if !visited.insert((
            without_fragment(&request.url),
            request.body.clone(),
            request.htmx,
        )) {
            continue;
        }
        requests += 1;
        if !request.wait.is_zero() {
            tokio::time::sleep(request.wait).await;
        }
        let response = match send(client, request).await {
            Ok(response) => response,
            Err(error) if step.depth == 0 => return Err(error),
            Err(_) => continue,
        };
        if step.depth == 0 {
            origins.push(site(response.url()));
        }
        let status = response.status();
        let current = response.url().clone();
        // A link may redirect off-site (`/out?u=…`); forms and htmx keep their CDN redirects.
        let trusted = step.trusted && (!step.markup_link || origins.contains(&site(&current)));
        let follow = |url: Url, score| Step {
            score,
            depth: step.depth + 1,
            trusted,
            markup_link: false,
            request: Request::get(url, Some(current.clone())),
        };

        if matches!(status.as_u16(), 403 | 503) && crate::probe::is_browser_challenge(&response) {
            if request.htmx {
                evidence.challenged = true;
            } else {
                // Cloudflare rules on htmx-built hosts exempt htmx requests.
                let request = Request {
                    htmx: true,
                    ..request.clone()
                };
                queue.push(Step { request, ..step }, &hint);
            }
            continue;
        }
        if status.is_success()
            && let Some(target) =
                htmx_target(&response).and_then(|target| current.join(&target).ok())
        {
            queue.push(follow(target, FORCED_SCORE), &hint);
            continue;
        }
        if !status.is_success() {
            if step.depth == 0 {
                return Err(crate::probe::http_error(&response));
            }
            continue;
        }

        if is_file(&response) {
            if let Some(resolved) = accept_file(&step, &response, trusted, &hint) {
                return Ok(resolved);
            }
            continue;
        }

        let refresh = response
            .headers()
            .get(header::REFRESH)
            .and_then(|value| value.to_str().ok())
            .and_then(page::refresh_url)
            .map(str::to_owned);
        let json = crate::probe::content_type_essence(response.headers()).as_deref()
            == Some("application/json");
        let body = match read_bounded(response).await {
            Ok(body) => body,
            Err(error) if step.depth == 0 => return Err(error),
            Err(_) => continue,
        };
        let page = tokio::task::spawn_blocking(move || {
            if json {
                let mut page = Page::default();
                page::script_links(&body, &mut page.links);
                page
            } else {
                page::parse(&body)
            }
        })
        .await
        .unwrap_or_default();

        if trusted {
            evidence.captcha |= page.captcha;
            evidence.password |= page
                .forms
                .iter()
                .any(|form| form.password && (form.xfs || words(&form.text) >= MIN_SCORE));
            hint.learn(page.title.as_deref());
        }
        if let Some(target) = refresh
            .as_deref()
            .or(page.refresh.as_deref())
            .and_then(|target| current.join(target).ok())
        {
            queue.push(follow(target, FORCED_SCORE), &hint);
        }
        for next in candidates(&page, &current, &hint, &origins, step.depth + 1) {
            queue.push(next, &hint);
        }
    }
    Err(evidence.into_error())
}

fn accept_file(
    step: &Step,
    response: &reqwest::Response,
    trusted: bool,
    hint: &Hint,
) -> Option<Resolved> {
    let request = &step.request;
    let disposition = response
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| parse_content_disposition(value).filename);
    let served = disposition
        .clone()
        .or_else(|| path_filename(response.url()))?;
    let fetched_by_get = request.body.is_none()
        || without_fragment(response.url()) != without_fragment(&request.url);
    let accepted = fetched_by_get
        && hint.allows(&served)
        && (trusted || hint.name_matches(&served))
        && (hint.name.is_some() || disposition.is_some());
    accepted.then(|| {
        let referer = request.referer.as_ref().unwrap_or_else(|| response.url());
        Resolved {
            url: response.url().to_string(),
            headers: vec![("Referer".to_owned(), referer.to_string())],
        }
    })
}

async fn send(client: &reqwest::Client, request: &Request) -> Result<reqwest::Response> {
    let url = without_fragment(&request.url);
    let mut builder = match &request.body {
        Some(body) => client
            .post(&url)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body.clone()),
        None => client.get(&url),
    };
    if let Some(referer) = &request.referer {
        builder = builder.header(header::REFERER, referer.as_str());
    }
    if request.htmx {
        let current = request.referer.as_ref().map_or(url.as_str(), Url::as_str);
        builder = builder
            .header("HX-Request", "true")
            .header("HX-Current-URL", current);
    }
    Ok(builder.send().await?)
}

async fn read_bounded(mut response: reqwest::Response) -> Result<String> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        let room = MAX_PAGE_BYTES - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if body.len() >= MAX_PAGE_BYTES {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

fn htmx_target(response: &reqwest::Response) -> Option<String> {
    let header = |name| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    if let Some(target) = header("hx-redirect") {
        return Some(target.to_owned());
    }
    let location = header("hx-location")?;
    if location.starts_with('{') {
        let value: serde_json::Value = serde_json::from_str(location).ok()?;
        return value.get("path")?.as_str().map(str::to_owned);
    }
    Some(location.to_owned())
}

fn is_file(response: &reqwest::Response) -> bool {
    let headers = response.headers();
    let attachment = headers
        .get(header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| parse_content_disposition(value).attachment);
    attachment
        || match crate::probe::content_type_essence(headers) {
            Some(essence) => !PAGE_TYPES.contains(&essence.as_str()),
            None => response.content_length().is_some_and(|length| length > 0),
        }
}

fn candidates(page: &Page, current: &Url, hint: &Hint, origins: &[String], depth: u8) -> Vec<Step> {
    let trusted = |url: &Url| origins.contains(&site(url));
    let mut steps = Vec::new();
    for link in &page.links {
        let Some(url) = current.join(&link.url).ok() else {
            continue;
        };
        let Some(base) = link_score(&url, current, hint) else {
            continue;
        };
        // Script URLs must stand on their own, then rank below equal markup links.
        let (score, kept) = if link.script {
            (base - 10, base)
        } else {
            let score = base + words(&link.text) + 40 * i32::from(link.download);
            (score, score)
        };
        if kept < MIN_SCORE {
            continue;
        }
        steps.push(Step {
            score,
            depth,
            trusted: trusted(&url),
            markup_link: true,
            request: Request::get(url, Some(current.clone())),
        });
    }
    for action in &page.actions {
        let Some(mut url) = current.join(&action.url).ok() else {
            continue;
        };
        let Some(base) = link_score(&url, current, hint) else {
            continue;
        };
        let score = 20 + words(&action.text) + base;
        if score < MIN_SCORE {
            continue;
        }
        let body = if action.post {
            Some(encode(&action.params))
        } else {
            if !action.params.is_empty() {
                url.query_pairs_mut().extend_pairs(&action.params);
            }
            None
        };
        steps.push(Step {
            score,
            depth,
            trusted: trusted(&url),
            markup_link: false,
            request: Request {
                url,
                body,
                htmx: true,
                referer: Some(current.clone()),
                wait: Duration::ZERO,
            },
        });
    }
    let wait = page
        .countdown
        .map(|seconds| Duration::from_secs(seconds.into()) + Duration::from_secs(1))
        .unwrap_or_default()
        .min(MAX_WAIT);
    for form in page
        .forms
        .iter()
        .filter(|form| !form.password && !form.captcha)
    {
        let url = if form.action.trim().is_empty() {
            let mut url = current.clone();
            url.set_fragment(None);
            Some(url)
        } else {
            current.join(form.action.trim()).ok()
        };
        let Some(mut url) = url.filter(|url| matches!(url.scheme(), "http" | "https")) else {
            continue;
        };
        let score = words(&form.text)
            + 40 * i32::from(form.xfs)
            + 20 * i32::from(form.fields.iter().any(|(name, _)| name == "method_free"))
            + 20 * i32::from(form.text.split(' ').any(|word| word == "F1"));
        if score < MIN_SCORE {
            continue;
        }
        let body = if form.post {
            Some(encode(&form.fields))
        } else {
            url.set_query(None);
            if !form.fields.is_empty() {
                url.query_pairs_mut().extend_pairs(&form.fields);
            }
            None
        };
        steps.push(Step {
            score,
            depth,
            trusted: trusted(&url),
            markup_link: false,
            request: Request {
                url,
                body,
                htmx: false,
                referer: Some(current.clone()),
                wait,
            },
        });
    }
    steps.retain(|step| step.trusted || hint.is_name(&step.request.url));
    steps.sort_by_key(|step| Reverse(step.score));
    steps.truncate(MAX_STEPS_PER_PAGE);
    steps
}

fn encode(fields: &[(String, String)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish()
}

fn words(text: &str) -> i32 {
    let text = text.to_lowercase();
    let mut score = 0;
    if text.contains("download") {
        score += 40;
    }
    if ["continue", "get link", "free", "start", "proceed"]
        .iter()
        .any(|word| text.contains(word))
    {
        score += 15;
    }
    if NAVIGATION_WORDS.iter().any(|word| text.contains(word)) {
        score -= 60;
    }
    score
}

fn link_score(url: &Url, page: &Url, hint: &Hint) -> Option<i32> {
    if !matches!(url.scheme(), "http" | "https") || without_fragment(url) == without_fragment(page)
    {
        return None;
    }
    let name = path_filename(url).unwrap_or_default();
    let extension = Hint::extension(&name).map(str::to_ascii_lowercase);
    if extension
        .as_deref()
        .is_some_and(|extension| ASSET_EXTENSIONS.contains(&extension))
    {
        return None;
    }
    let mut score = 0;
    if hint.name_matches(&name) {
        score += 100;
    } else if hint
        .stem()
        .is_some_and(|stem| name.to_lowercase().contains(&stem.to_lowercase()))
    {
        score += 40;
    }
    if extension
        .as_deref()
        .is_some_and(|extension| FILE_EXTENSIONS.contains(&extension))
        || rar_part(&name).is_some()
        || sevenz_volume(&name).is_some()
    {
        score += 30;
    }
    if url.as_str().to_ascii_lowercase().contains("download") {
        score += 40;
    }
    if url.path_segments().is_some_and(|mut segments| {
        segments.any(|segment| ["dl", "d", "get", "file", "files"].contains(&segment))
    }) {
        score += 20;
    }
    let path = url.path().to_ascii_lowercase();
    if NAVIGATION_WORDS.iter().any(|word| path.contains(word)) {
        score -= 60;
    }
    Some(score)
}

fn without_fragment(url: &Url) -> String {
    let mut url = url.clone();
    url.set_fragment(None);
    url.into()
}

fn site(url: &Url) -> String {
    match url.host() {
        Some(url::Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            let labels: Vec<&str> = domain.split('.').collect();
            let count = labels.len();
            // Country registries' second levels (`co.uk`, `com.au`), not short names like `abc.to`.
            let registry = count >= 3
                && labels[count - 1].len() == 2
                && SECOND_LEVEL_SUFFIXES.contains(&labels[count - 2]);
            let keep = if registry { 3 } else { 2 };
            labels[count.saturating_sub(keep)..].join(".")
        },
        Some(host) => host.to_string(),
        None => String::new(),
    }
}

#[derive(Default)]
struct Hint {
    name: Option<String>,
}

impl Hint {
    fn from_url(url: &Url) -> Self {
        let fragment = fragment_name(url).filter(|name| name.contains('.'));
        let name = fragment.or_else(|| {
            let segment = path_filename(url)?;
            let lower = segment.to_ascii_lowercase();
            let stem = [".html", ".htm", ".php"]
                .iter()
                .find(|suffix| lower.ends_with(*suffix))
                .map_or(segment.as_str(), |suffix| {
                    &segment[..segment.len() - suffix.len()]
                });
            stem.contains('.').then(|| stem.to_owned())
        });
        Self { name }
    }

    fn learn(&mut self, title: Option<&str>) {
        if self.name.is_some() {
            return;
        }
        let Some(title) = title else {
            return;
        };
        let end = [" - ", " | "]
            .iter()
            .filter_map(|separator| title.find(separator))
            .min()
            .unwrap_or(title.len());
        let name = title[..end].trim();
        let valid = !name.contains('/')
            && name.rsplit_once('.').is_some_and(|(stem, extension)| {
                !stem.is_empty()
                    && (1..=5).contains(&extension.len())
                    && extension.bytes().all(|b| b.is_ascii_alphanumeric())
            });
        if valid {
            self.name = Some(name.to_owned());
        }
    }

    fn extension(name: &str) -> Option<&str> {
        name.rsplit_once('.').map(|(_, extension)| extension)
    }

    fn stem(&self) -> Option<&str> {
        let name = self.name.as_deref()?;
        let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
        (stem.len() >= 3).then_some(stem)
    }

    fn name_matches(&self, served: &str) -> bool {
        self.name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(served))
    }

    fn is_name(&self, url: &Url) -> bool {
        path_filename(url).is_some_and(|name| self.name_matches(&name))
    }

    fn allows(&self, served: &str) -> bool {
        match self.name.as_deref().and_then(Self::extension) {
            Some(expected) => {
                Self::extension(served).is_some_and(|served| served.eq_ignore_ascii_case(expected))
            },
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(url: &str, text: &str, script: bool) -> page::Link {
        page::Link {
            url: url.into(),
            text: text.into(),
            download: false,
            script,
        }
    }

    fn page(links: Vec<page::Link>) -> Page {
        Page {
            links,
            ..Page::default()
        }
    }

    fn steps(page_url: &str, hint: &Hint, links: Vec<page::Link>) -> Vec<String> {
        let current = Url::parse(page_url).unwrap();
        candidates(&page(links), &current, hint, &[site(&current)], 1)
            .into_iter()
            .map(|step| step.request.url.to_string())
            .collect()
    }

    #[test]
    fn candidates_are_the_file_not_navigation_or_other_sites() {
        let hint = Hint {
            name: Some("x.part1.rar".into()),
        };
        let urls = steps(
            "https://x.example/page",
            &hint,
            vec![
                link("/premium", "Premium", false),
                link("/files/x.part1.rar", "", false),
                link("/help", "", false),
                link("https://ads.example/download-now", "Download", false),
            ],
        );
        assert_eq!(urls, ["https://x.example/files/x.part1.rar"]);

        let urls = steps(
            "https://x.example/file/d/1/view",
            &Hint::default(),
            vec![link("https://x.example/uc?id=1&export=download", "", true)],
        );
        assert_eq!(urls, ["https://x.example/uc?id=1&export=download"]);
    }

    #[test]
    fn hints_come_from_fragments_paths_and_titles() {
        for (url, expected) in [
            (
                "https://h.example/abc#a%20b.part1.rar",
                Some("a b.part1.rar"),
            ),
            ("https://h.example/abc/file.zip.html", Some("file.zip")),
            ("https://h.example/download.php?id=1", None),
            ("https://h.example/abc", None),
        ] {
            let hint = Hint::from_url(&Url::parse(url).unwrap());
            assert_eq!(hint.name.as_deref(), expected, "{url}");
        }
        for (title, expected) in [
            ("movie.mkv - Host", Some("movie.mkv")),
            ("Host | Free file sharing", None),
            ("a/b.zip", None),
        ] {
            let mut hint = Hint::default();
            hint.learn(Some(title));
            assert_eq!(hint.name.as_deref(), expected, "{title}");
        }
    }

    #[test]
    fn sites_group_subdomains() {
        for (url, expected) in [
            ("https://dl.example.co/x", "example.co"),
            ("https://a.b.example.co.uk/x", "example.co.uk"),
            ("https://dl.abc.to/x", "abc.to"),
            ("http://127.0.0.1:8080/x", "127.0.0.1"),
        ] {
            assert_eq!(site(&Url::parse(url).unwrap()), expected, "{url}");
        }
    }
}
