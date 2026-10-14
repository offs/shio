use std::cell::RefCell;
use std::collections::HashSet;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};

const MAX_LINKS: usize = 500;
const MAX_ACTIONS: usize = 50;
const MAX_FORMS: usize = 20;
const MAX_FIELDS: usize = 50;
// App-shell pages (Google Drive) put the file URL after ~70 KiB of inline script.
const MAX_SCRIPT_BYTES: usize = 512 * 1024;
const MAX_TEXT_CHARS: usize = 200;
const MAX_LITERAL_BYTES: usize = 2048;
const COUNTDOWN_TOKENS: u8 = 6;
const MAX_COUNTDOWN_SECS: u32 = 300;
const CAPTCHA_MARKERS: [&str; 3] = ["g-recaptcha", "h-captcha", "cf-turnstile"];
const CAPTCHA_SCRIPTS: [&str; 3] = [
    "challenges.cloudflare.com/turnstile",
    "hcaptcha.com/1/api.js",
    "recaptcha/api.js",
];
const NAVIGATIONS: [&str; 5] = [
    "location.href",
    "location.assign(",
    "location.replace(",
    "location =",
    "window.open(",
];

#[derive(Default)]
pub(super) struct Page {
    pub(super) title: Option<String>,
    pub(super) refresh: Option<String>,
    pub(super) countdown: Option<u32>,
    pub(super) captcha: bool,
    pub(super) links: Vec<Link>,
    pub(super) actions: Vec<Action>,
    pub(super) forms: Vec<Form>,
}

pub(super) struct Link {
    pub(super) url: String,
    pub(super) text: String,
    pub(super) download: bool,
    pub(super) script: bool,
}

pub(super) struct Action {
    pub(super) post: bool,
    pub(super) url: String,
    pub(super) params: Vec<(String, String)>,
    pub(super) text: String,
}

pub(super) struct Form {
    pub(super) post: bool,
    pub(super) action: String,
    pub(super) fields: Vec<(String, String)>,
    pub(super) text: String,
    pub(super) xfs: bool,
    pub(super) password: bool,
    pub(super) captcha: bool,
}

pub(super) fn parse(html: &str) -> Page {
    let tokenizer = Tokenizer::new(Sink::default(), TokenizerOpts::default());
    let queue = BufferQueue::default();
    queue.push_back(StrTendril::from_slice(html));
    let _ = tokenizer.feed(&queue);
    tokenizer.end();
    let mut page = tokenizer.sink.page.take();
    tokenizer.sink.state.take().finish(&mut page);
    page
}

pub(super) fn script_links(text: &str, out: &mut Vec<Link>) {
    let text = unescape(text);
    let mut seen = HashSet::new();
    let mut push = |url: &str| {
        if out.len() < MAX_LINKS && seen.insert(url.to_owned()) {
            out.push(Link {
                url: url.to_owned(),
                text: String::new(),
                download: false,
                script: true,
            });
        }
    };
    let mut position = 0;
    while let Some(found) = text[position..].find("http") {
        let start = position + found;
        let rest = &text[start..];
        position = start + 4;
        let Some(scheme) = ["https://", "http://"]
            .into_iter()
            .find(|scheme| rest.starts_with(scheme))
        else {
            continue;
        };
        let end = rest
            .find(|c: char| c.is_whitespace() || "\"'<>\\`)]};".contains(c))
            .unwrap_or(rest.len());
        if end > scheme.len() {
            push(&rest[..end]);
            position = start + end;
        }
    }
    for navigation in NAVIGATIONS {
        for (start, _) in text.match_indices(navigation) {
            if let Some(literal) = next_literal(&text[start + navigation.len()..]) {
                push(literal);
            }
        }
    }
}

pub(super) fn refresh_url(content: &str) -> Option<&str> {
    let index = content.to_ascii_lowercase().find("url=")?;
    let url = content[index + 4..]
        .trim()
        .trim_matches(|c| matches!(c, '"' | '\''))
        .trim();
    (!url.is_empty()).then_some(url)
}

fn next_literal(text: &str) -> Option<&str> {
    let start = text.find(|c: char| !(c.is_whitespace() || c == '=' || c == '('))?;
    let quote = text[start..]
        .chars()
        .next()
        .filter(|c| matches!(c, '"' | '\''))?;
    let body = &text[start + 1..];
    let end = body
        .find(quote)
        .filter(|&end| end > 0 && end <= MAX_LITERAL_BYTES)?;
    Some(&body[..end])
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('\\') {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        let escape = tail
            .get(..6)
            .and_then(|escape| match escape.to_ascii_lowercase().as_str() {
                "\\u0026" => Some('&'),
                "\\u003d" => Some('='),
                "\\u002f" => Some('/'),
                "\\u003f" => Some('?'),
                _ => None,
            });
        let (replacement, length) = match escape {
            Some(c) => (c, 6),
            None if tail.starts_with("\\/") => ('/', 2),
            None => ('\\', 1),
        };
        out.push(replacement);
        rest = &tail[length..];
    }
    out.push_str(rest);
    out
}

#[derive(Default)]
struct Sink {
    page: RefCell<Page>,
    state: RefCell<State>,
}

impl TokenSink for Sink {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
        self.state
            .borrow_mut()
            .token(&mut self.page.borrow_mut(), token)
    }
}

#[derive(Clone, Copy)]
enum Raw {
    Script,
    Title,
    Ignored,
}

#[derive(Clone, Copy)]
enum Capture {
    Link(usize),
    Action(usize),
    Form(usize),
}

#[derive(Default)]
struct State {
    raw: Option<Raw>,
    capture: Option<(String, Vec<Capture>)>,
    form: Option<usize>,
    form_submitted: bool,
    script: String,
    title: String,
    countdown: u8,
}

impl State {
    fn token(&mut self, page: &mut Page, token: Token) -> TokenSinkResult<()> {
        match token {
            Token::TagToken(tag) => match tag.kind {
                TagKind::StartTag => {
                    self.countdown = self.countdown.saturating_sub(1);
                    return self.start(page, &tag);
                },
                TagKind::EndTag => self.end(page, &tag.name),
            },
            Token::CharacterTokens(text) => self.text(page, &text),
            _ => {},
        }
        TokenSinkResult::Continue
    }

    fn start(&mut self, page: &mut Page, tag: &Tag) -> TokenSinkResult<()> {
        let name = &*tag.name;
        self.mark_captcha(page, tag);
        if ["id", "class"].iter().any(|key| {
            attr(tag, key).is_some_and(|value| {
                let value = value.to_ascii_lowercase();
                ["countdown", "seconds", "timer"]
                    .iter()
                    .any(|marker| value.contains(marker))
            })
        }) {
            self.countdown = COUNTDOWN_TOKENS;
        }
        match name {
            "script" => {
                self.raw = Some(Raw::Script);
                return TokenSinkResult::RawData(RawKind::ScriptData);
            },
            "style" | "iframe" | "noembed" | "noframes" | "xmp" => {
                self.raw = Some(Raw::Ignored);
                return TokenSinkResult::RawData(RawKind::Rawtext);
            },
            "title" | "textarea" => {
                self.raw = Some(if name == "title" {
                    Raw::Title
                } else {
                    Raw::Ignored
                });
                return TokenSinkResult::RawData(RawKind::Rcdata);
            },
            "meta" => Self::meta(page, tag),
            "form" => self.open_form(page, tag),
            "input" => self.input(page, tag),
            _ => {},
        }

        let mut targets = Vec::new();
        if matches!(name, "a" | "area")
            && let Some(href) = attr(tag, "href")
            && page.links.len() < MAX_LINKS
        {
            let mut text = String::new();
            for key in ["id", "class", "title"] {
                push_text(&mut text, attr(tag, key).unwrap_or_default());
            }
            page.links.push(Link {
                url: href.to_owned(),
                text,
                download: attr(tag, "download").is_some(),
                script: false,
            });
            targets.push(Capture::Link(page.links.len() - 1));
        }
        if let Some(action) = self.action(page, tag) {
            page.actions.push(action);
            targets.push(Capture::Action(page.actions.len() - 1));
        }
        if name == "button"
            && let Some(index) = self.form
        {
            if let Some(field) = attr(tag, "name")
                && !std::mem::replace(&mut self.form_submitted, true)
            {
                add_field(
                    &mut page.forms[index],
                    field,
                    attr(tag, "value").unwrap_or_default(),
                );
            }
            targets.push(Capture::Form(index));
        }
        if !targets.is_empty() && !tag.self_closing && name != "area" {
            self.capture = Some((name.to_owned(), targets));
        }
        TokenSinkResult::Continue
    }

    fn end(&mut self, page: &mut Page, name: &str) {
        match self.raw.take() {
            Some(Raw::Script) => {
                if self.script.len() < MAX_SCRIPT_BYTES {
                    self.script.push('\n');
                }
                return;
            },
            Some(Raw::Title) => {
                let title = std::mem::take(&mut self.title);
                if page.title.is_none() && !title.is_empty() {
                    page.title = Some(title);
                }
                return;
            },
            Some(Raw::Ignored) => return,
            None => {},
        }
        if self.capture.as_ref().is_some_and(|(tag, _)| tag == name) {
            self.capture = None;
        }
        if name == "form" {
            self.close_form(page);
        }
    }

    fn text(&mut self, page: &mut Page, text: &str) {
        match self.raw {
            Some(Raw::Script) => {
                let room = MAX_SCRIPT_BYTES.saturating_sub(self.script.len());
                let mut end = text.len().min(room);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                self.script.push_str(&text[..end]);
                return;
            },
            Some(Raw::Title) => {
                push_text(&mut self.title, text);
                return;
            },
            Some(Raw::Ignored) => return,
            None => {},
        }
        if let Some((_, targets)) = &self.capture {
            for target in targets {
                let text_field = match *target {
                    Capture::Link(index) => &mut page.links[index].text,
                    Capture::Action(index) => &mut page.actions[index].text,
                    Capture::Form(index) => &mut page.forms[index].text,
                };
                push_text(text_field, text);
            }
        }
        if self.countdown > 0 {
            self.countdown -= 1;
            if let Some(seconds) = first_number(text).filter(|&s| s <= MAX_COUNTDOWN_SECS) {
                page.countdown = Some(page.countdown.map_or(seconds, |max| max.max(seconds)));
                self.countdown = 0;
            }
        }
    }

    fn mark_captcha(&self, page: &mut Page, tag: &Tag) {
        let captcha = tag.attrs.iter().any(|attribute| {
            let value = attribute.value.to_ascii_lowercase();
            &*attribute.name.local == "data-sitekey"
                || CAPTCHA_MARKERS.iter().any(|marker| value.contains(marker))
                || (&*tag.name == "script"
                    && &*attribute.name.local == "src"
                    && CAPTCHA_SCRIPTS.iter().any(|script| value.contains(script)))
        });
        if captcha {
            page.captcha = true;
            if let Some(index) = self.form {
                page.forms[index].captcha = true;
            }
        }
    }

    fn meta(page: &mut Page, tag: &Tag) {
        let content = attr(tag, "content").unwrap_or_default();
        if attr(tag, "property") == Some("og:title") && !content.trim().is_empty() {
            page.title = Some(content.trim().to_owned());
        }
        if attr(tag, "http-equiv").is_some_and(|value| value.eq_ignore_ascii_case("refresh"))
            && let Some(url) = refresh_url(content)
        {
            page.refresh = Some(url.to_owned());
        }
    }

    fn action(&self, page: &Page, tag: &Tag) -> Option<Action> {
        if page.actions.len() >= MAX_ACTIONS {
            return None;
        }
        let (post, url) = [
            ("hx-get", false),
            ("data-hx-get", false),
            ("hx-post", true),
            ("data-hx-post", true),
        ]
        .into_iter()
        .find_map(|(key, post)| attr(tag, key).map(|url| (post, url.to_owned())))?;
        let mut params = attr(tag, "hx-vals")
            .or_else(|| attr(tag, "data-hx-vals"))
            .and_then(hx_vals)
            .unwrap_or_default();
        if let Some(index) = self.form {
            params.extend(page.forms[index].fields.iter().cloned());
        }
        let mut text = String::new();
        for key in ["id", "class"] {
            push_text(&mut text, attr(tag, key).unwrap_or_default());
        }
        Some(Action {
            post,
            url,
            params,
            text,
        })
    }

    fn open_form(&mut self, page: &mut Page, tag: &Tag) {
        if self.form.is_some() || page.forms.len() >= MAX_FORMS {
            return;
        }
        let mut text = String::new();
        for key in ["id", "name", "class"] {
            push_text(&mut text, attr(tag, key).unwrap_or_default());
        }
        page.forms.push(Form {
            post: attr(tag, "method").is_some_and(|method| method.eq_ignore_ascii_case("post")),
            action: attr(tag, "action").unwrap_or_default().to_owned(),
            fields: Vec::new(),
            text,
            xfs: false,
            password: false,
            captcha: false,
        });
        self.form = Some(page.forms.len() - 1);
        self.form_submitted = false;
    }

    fn input(&mut self, page: &mut Page, tag: &Tag) {
        let Some(index) = self.form else {
            return;
        };
        let form = &mut page.forms[index];
        let value = attr(tag, "value").unwrap_or_default();
        let sent = match attr(tag, "type")
            .unwrap_or("text")
            .to_ascii_lowercase()
            .as_str()
        {
            "password" => {
                form.password = true;
                false
            },
            "submit" => {
                let first = !std::mem::replace(&mut self.form_submitted, true);
                if first {
                    push_text(&mut form.text, value);
                }
                first
            },
            "checkbox" | "radio" => attr(tag, "checked").is_some(),
            "file" | "image" | "reset" | "button" => false,
            _ => true,
        };
        if sent && let Some(name) = attr(tag, "name") {
            add_field(form, name, value);
        }
    }

    fn close_form(&mut self, page: &mut Page) {
        let Some(index) = self.form.take() else {
            return;
        };
        let form = &mut page.forms[index];
        form.fields.retain(|(name, _)| name != "method_premium");
        for (name, value) in &mut form.fields {
            if name == "method_free" && value.is_empty() {
                "Free Download".clone_into(value);
            }
        }
    }

    fn finish(mut self, page: &mut Page) {
        self.close_form(page);
        script_links(&self.script, &mut page.links);
    }
}

fn add_field(form: &mut Form, name: &str, value: &str) {
    if form.fields.len() >= MAX_FIELDS {
        return;
    }
    if name == "op" && value.starts_with("download") {
        form.xfs = true;
    }
    form.fields.push((name.to_owned(), value.to_owned()));
}

fn attr<'a>(tag: &'a Tag, name: &str) -> Option<&'a str> {
    tag.attrs
        .iter()
        .find(|attribute| &*attribute.name.local == name)
        .map(|attribute| &*attribute.value)
}

fn hx_vals(vals: &str) -> Option<Vec<(String, String)>> {
    let serde_json::Value::Object(map) = serde_json::from_str(vals).ok()? else {
        return None;
    };
    map.into_iter()
        .filter_map(|(key, value)| match value {
            serde_json::Value::String(value) if value.starts_with("js:") => Some(None),
            serde_json::Value::String(value) => Some(Some((key, value))),
            serde_json::Value::Number(value) => Some(Some((key, value.to_string()))),
            serde_json::Value::Bool(value) => Some(Some((key, value.to_string()))),
            _ => None,
        })
        .collect()
}

fn push_text(text: &mut String, chunk: &str) {
    for word in chunk.split_whitespace() {
        let used = text.chars().count();
        if used >= MAX_TEXT_CHARS {
            return;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.extend(word.chars().take(MAX_TEXT_CHARS.saturating_sub(used + 1)));
    }
}

fn first_number(text: &str) -> Option<u32> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let digits = &text[start..];
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_facts_come_from_markup_not_script_text() {
        let page = parse(
            r#"<html><head><title>x</title></head><body>
            <a class="link-button" hx-post="/f/abc123/go">DOWNLOAD</a>
            <form name="F1" method="POST" action="">
              <input type="hidden" name="op" value="download2">
              <input type="hidden" name="id" value="abc123">
              <input type="hidden" name="method_free" value="">
              <input type="hidden" name="method_premium" value="">
              <span id="countdown">Wait <span class="seconds">6</span> seconds</span>
            </form>
            <form id="download-form" method="get" action="https://files.example/download">
              <input type="hidden" name="confirm" value="t">
              <input type="submit" value="Download anyway">
            </form>
            <a href="/f.zip?a=1&amp;b=2" download>file</a>
            <script>if(a<b){} var u="https:\/\/cdn.example\/x.zip?a\u0026b"; location.href='/dl/abc'; document.write('<a href="/inside">x</a><form>');</script>
            <form method="post"><div class="cf-turnstile" data-sitekey="k"></div><button>Download</button></form>
            </body></html>"#,
        );

        let [action] = page.actions.as_slice() else {
            panic!("one htmx action");
        };
        assert!(action.post);
        assert_eq!(action.url, "/f/abc123/go");
        assert!(action.text.contains("DOWNLOAD"), "{}", action.text);

        let [xfs, drive, turnstile] = page.forms.as_slice() else {
            panic!("three forms");
        };
        assert!(xfs.xfs && xfs.post);
        assert!(
            xfs.fields
                .contains(&("method_free".into(), "Free Download".into()))
        );
        assert!(!xfs.fields.iter().any(|(name, _)| name == "method_premium"));
        assert_eq!(page.countdown, Some(6));
        assert!(!drive.post);
        assert_eq!(drive.fields, [("confirm".to_owned(), "t".to_owned())]);
        assert!(turnstile.captcha && !xfs.captcha && page.captcha);

        let urls: Vec<_> = page
            .links
            .iter()
            .map(|link| (link.url.as_str(), link.download, link.script))
            .collect();
        assert_eq!(
            urls,
            [
                ("/f.zip?a=1&b=2", true, false),
                ("https://cdn.example/x.zip?a&b", false, true),
                ("/dl/abc", false, true),
            ]
        );
    }

    #[test]
    fn meta_refresh_and_og_title_are_read() {
        let page = parse(
            r#"<title>Site name</title><meta property="og:title" content="file.part1.rar">
            <meta http-equiv="Refresh" content="3; URL='/next?id=1'">"#,
        );
        assert_eq!(page.title.as_deref(), Some("file.part1.rar"));
        assert_eq!(page.refresh.as_deref(), Some("/next?id=1"));

        let page = parse(
            "<head><title>\n  My   file.zip - Host\n</title></head><svg><title>Close</title></svg>",
        );
        assert_eq!(page.title.as_deref(), Some("My file.zip - Host"));
        assert_eq!(page.refresh, None);
    }
}
