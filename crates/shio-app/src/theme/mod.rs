use crate::style::{DANGER_ALPHA, Palette, SOFT_ALPHA, alpha, selection};
use iced::Color;
use serde::Deserialize;
use std::ffi::OsString;
use std::io::Read as _;
use std::path::Path;
use std::time::SystemTime;

const BUILT_IN_THEMES: [&str; 8] = [
    include_str!("../../../../assets/themes/dark.toml"),
    include_str!("../../../../assets/themes/light.toml"),
    include_str!("../../../../assets/themes/dawn.toml"),
    include_str!("../../../../assets/themes/catppuccin.toml"),
    include_str!("../../../../assets/themes/tokyo-night.toml"),
    include_str!("../../../../assets/themes/rose-pine.toml"),
    include_str!("../../../../assets/themes/kanagawa.toml"),
    include_str!("../../../../assets/themes/flexoki-light.toml"),
];
const MIN_TEXT_CONTRAST: f32 = 4.5;
const MIN_CONTROL_CONTRAST: f32 = 3.0;
const MAX_USER_THEME_BYTES: u64 = 16 * 1024;
const MAX_USER_THEME_ENTRIES: usize = 256;
const MAX_USER_THEMES: usize = 64;
const MAX_REPAIR_STEPS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ThemeId(String);

impl Default for ThemeId {
    fn default() -> Self {
        Self("dark".to_owned())
    }
}

impl ThemeId {
    pub(crate) fn parse(value: &str) -> Result<Self, ThemeError> {
        shio_core::validate_theme_id(value).map_err(|_| ThemeError::InvalidId(value.to_owned()))?;
        Ok(Self(value.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for ThemeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Appearance {
    Light,
    Dark,
}

#[derive(Debug, Clone)]
pub(crate) struct ThemeDefinition {
    pub(crate) id: ThemeId,
    pub(crate) name: String,
    appearance: Appearance,
    colors: Colors,
}

#[derive(Debug, Clone, Copy)]
struct Colors {
    background: Color,
    text_primary: Color,
    text_secondary: Color,
    text_tertiary: Color,
    accent: Color,
    success: Color,
    warning: Color,
    error: Color,
    border: Color,
}

#[derive(Debug, Clone)]
pub(crate) struct ThemeCatalog {
    themes: Vec<ThemeDefinition>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedTheme {
    pub(crate) id: ThemeId,
    pub(crate) palette: Palette,
    pub(crate) iced_theme: iced::Theme,
}

#[derive(Debug)]
pub(crate) enum ThemeError {
    Parse(toml::de::Error),
    InvalidId(String),
    InvalidColor { field: &'static str, value: String },
    LowContrast { pair: &'static str, ratio: f32 },
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "parse error: {e}"),
            Self::InvalidId(id) => write!(f, "invalid id: {id}"),
            Self::InvalidColor { field, value } => write!(f, "invalid color {field}: {value}"),
            Self::LowContrast { pair, ratio } => {
                write!(
                    f,
                    "{pair} contrast is {ratio:.2}:1, below the required minimum"
                )
            },
        }
    }
}

impl ThemeCatalog {
    pub(crate) fn load(user_theme_dir: &Path) -> Self {
        let mut catalog = Self::built_in();
        let entries = match std::fs::read_dir(user_theme_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return catalog,
            Err(e) => {
                let dir = user_theme_dir.display();
                tracing::warn!("failed to read theme directory {dir}: {e}");
                return catalog;
            },
        };

        let mut loaded = 0;
        for (index, entry) in entries.enumerate() {
            if index >= MAX_USER_THEME_ENTRIES || loaded >= MAX_USER_THEMES {
                tracing::warn!("theme directory limit reached; ignoring the rest");
                break;
            }
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("toml") {
                continue;
            }
            if !entry.file_type().is_ok_and(|file_type| file_type.is_file()) {
                tracing::warn!("theme is not a regular file: {}", path.display());
                continue;
            }
            let parsed = read_user_theme(&path)
                .map_err(|e| e.to_string())
                .and_then(|content| ThemeDefinition::parse(&content).map_err(|e| e.to_string()));
            match parsed {
                Ok(definition) if catalog.find(&definition.id).is_some() => {
                    tracing::warn!(
                        "skipping theme {}: duplicate id {}",
                        path.display(),
                        definition.id
                    );
                },
                Ok(definition) => {
                    catalog.themes.push(definition);
                    loaded += 1;
                },
                Err(e) => tracing::warn!("skipping theme {}: {e}", path.display()),
            }
        }
        catalog
    }

    fn built_in() -> Self {
        let themes = BUILT_IN_THEMES
            .iter()
            .map(|content| {
                ThemeDefinition::parse(content)
                    .unwrap_or_else(|e| panic!("built-in theme must validate: {e}"))
            })
            .collect();
        Self { themes }
    }

    pub(crate) fn themes(&self) -> &[ThemeDefinition] {
        &self.themes
    }

    pub(crate) fn resolve(&self, id: &ThemeId, blur: bool) -> ResolvedTheme {
        let definition = self.find(id).unwrap_or_else(|| {
            tracing::warn!("theme '{id}' not found; using dark");
            self.find(&ThemeId::default())
                .expect("the built-in dark theme is always in the catalog")
        });
        let palette = definition.palette(blur);
        let iced_theme = iced::Theme::custom(
            definition.id.to_string(),
            iced::theme::Palette {
                background: palette.bg_base,
                text: palette.text_primary,
                primary: palette.accent,
                success: palette.success,
                warning: palette.warning,
                danger: palette.error,
            },
        );
        ResolvedTheme {
            id: definition.id.clone(),
            palette,
            iced_theme,
        }
    }

    pub(crate) fn find(&self, id: &ThemeId) -> Option<&ThemeDefinition> {
        self.themes.iter().find(|definition| definition.id == *id)
    }
}

// Length catches an in-place rewrite within one coarse mtime tick.
pub(crate) fn dir_stamp(user_theme_dir: &Path) -> Vec<(OsString, Option<SystemTime>, u64)> {
    let Ok(entries) = std::fs::read_dir(user_theme_dir) else {
        return Vec::new();
    };
    let mut stamp: Vec<_> = entries
        .take(MAX_USER_THEME_ENTRIES)
        .flatten()
        .map(|entry| {
            let metadata = entry.metadata().ok();
            (
                entry.file_name(),
                metadata.as_ref().and_then(|m| m.modified().ok()),
                metadata.map_or(0, |m| m.len()),
            )
        })
        .collect();
    stamp.sort_unstable();
    stamp
}

impl ThemeDefinition {
    fn parse(content: &str) -> Result<Self, ThemeError> {
        let raw: RawTheme = toml::from_str(content).map_err(ThemeError::Parse)?;
        let id = ThemeId::parse(&raw.id)?;
        let dark = raw.appearance == Appearance::Dark;
        Ok(Self {
            name: raw.name.unwrap_or_else(|| raw.id.clone()),
            id,
            appearance: raw.appearance,
            colors: Colors::try_from(raw.colors)?.readable(dark)?,
        })
    }

    pub(crate) const fn swatch(&self) -> [Color; 3] {
        [
            self.colors.background,
            self.colors.text_primary,
            self.colors.accent,
        ]
    }

    fn palette(&self, blur: bool) -> Palette {
        let c = self.colors;
        let dark = self.appearance == Appearance::Dark;
        let ink = if dark { Color::WHITE } else { Color::BLACK };
        let raised = c.raised(dark);
        let tint = if blur { c.tint_alpha(dark) } else { 1.0 };
        let [black, black_lit, white, white_lit] = c.base_surfaces(dark, tint);
        let buttons = [c.background, raised, black, black_lit, white, white_lit].map(|s| c.soft(s));
        Palette {
            dark,
            bg_base: alpha(c.background, tint),
            raised,
            raised_border: composite(alpha(c.border, if dark { 0.14 } else { 0.16 }), raised),
            text_primary: c.text_primary,
            text_secondary: c.text_secondary,
            text_tertiary: c.text_tertiary,
            accent: c.accent,
            accent_text: c.accent_text(&buttons),
            on_accent: c.on_accent(),
            success: c.success,
            warning: c.warning,
            error: c.error,
            on_error: ink_on(c.error),
            hover: alpha(ink, if dark { 0.05 } else { 0.04 }),
            pressed: alpha(ink, if dark { 0.09 } else { 0.07 }),
        }
    }
}

fn ink_on(background: Color) -> Color {
    if contrast_ratio(Color::WHITE, background) >= contrast_ratio(Color::BLACK, background) {
        Color::WHITE
    } else {
        Color::BLACK
    }
}

fn readable(text: Color, on: Color) -> bool {
    contrast_ratio(text, on) >= MIN_TEXT_CONTRAST
}

type Field = fn(&mut Colors) -> &mut Color;

struct Failure {
    pair: &'static str,
    ratio: f32,
    field: Field,
}

impl Colors {
    // Generated palettes (Noctalia) rarely pass as-is, so failing colours are moved toward
    // whichever of white or black contrasts more with the background.
    fn readable(mut self, dark: bool) -> Result<Self, ThemeError> {
        let ink = ink_on(self.background);
        for _ in 0..MAX_REPAIR_STEPS {
            let Err(failure) = self.validate(dark) else {
                return Ok(self);
            };
            let color = (failure.field)(&mut self);
            *color = mix(*color, ink, 0.1);
        }
        self.validate(dark)
            .map(|()| self)
            .map_err(|failure| ThemeError::LowContrast {
                pair: failure.pair,
                ratio: failure.ratio,
            })
    }

    fn validate(&self, dark: bool) -> Result<(), Failure> {
        let raised = self.raised(dark);
        let surfaces = [self.background, raised];
        let text: [(&'static str, Color, Field); 3] = [
            ("primary text", self.text_primary, |c| &mut c.text_primary),
            ("secondary text", self.text_secondary, |c| {
                &mut c.text_secondary
            }),
            ("tertiary text", self.text_tertiary, |c| {
                &mut c.text_tertiary
            }),
        ];
        let semantic: [(&'static str, Color, Field); 4] = [
            ("accent", self.accent, |c| &mut c.accent),
            ("success", self.success, |c| &mut c.success),
            ("warning", self.warning, |c| &mut c.warning),
            ("error", self.error, |c| &mut c.error),
        ];

        let mut checks = Vec::with_capacity(24);
        for (pair, color, field) in text.into_iter().chain(semantic) {
            checks.push((pair, color, self.background, MIN_TEXT_CONTRAST, field));
        }
        for (pair, color, field) in text.into_iter().chain([semantic[3]]) {
            checks.push((pair, color, raised, MIN_TEXT_CONTRAST, field));
        }
        for (pair, color, field) in &text[..2] {
            for surface in surfaces {
                let selected = composite(selection(dark), surface);
                checks.push((pair, *color, selected, MIN_TEXT_CONTRAST, *field));
            }
        }
        let buttons = [self.soft(self.background), self.soft(raised)];
        let accent_text = self.accent_text(&buttons);
        for fill in buttons {
            let field: Field = |c| &mut c.text_primary;
            checks.push(("accent button", accent_text, fill, MIN_TEXT_CONTRAST, field));
        }
        let danger = composite(alpha(self.error, DANGER_ALPHA), raised);
        checks.push((
            "danger button",
            self.error,
            danger,
            MIN_TEXT_CONTRAST,
            |c| &mut c.error,
        ));
        checks.push((
            "toggle knob",
            self.on_accent(),
            self.accent,
            MIN_CONTROL_CONTRAST,
            |c| &mut c.accent,
        ));

        checks
            .into_iter()
            .find_map(|(pair, foreground, background, minimum, field)| {
                let ratio = contrast_ratio(foreground, background);
                (ratio < minimum).then_some(Failure { pair, ratio, field })
            })
            .map_or(Ok(()), Err)
    }

    // Popups stay opaque: the compositor only blurs behind the window, so the rows beneath a
    // translucent popup would show through as sharp ghost text.
    fn raised(&self, dark: bool) -> Color {
        if !dark {
            return mix(self.background, Color::WHITE, 0.6);
        }
        (0_u8..=14)
            .rev()
            .map(|step| mix(self.background, self.text_primary, f32::from(step) / 100.0))
            .find(|&fill| {
                let lit = composite(selection(true), fill);
                [
                    self.text_primary,
                    self.text_secondary,
                    self.text_tertiary,
                    self.error,
                ]
                .into_iter()
                .all(|text| readable(text, fill))
                    && readable(self.text_primary, lit)
                    && readable(self.text_secondary, lit)
                    && readable(self.error, composite(alpha(self.error, DANGER_ALPHA), fill))
            })
            .unwrap_or(self.background)
    }

    fn on_accent(&self) -> Color {
        [self.background, Color::WHITE, Color::BLACK]
            .into_iter()
            .find(|&color| readable(color, self.accent))
            .unwrap_or(Color::BLACK)
    }

    fn soft(&self, under: Color) -> Color {
        composite(alpha(self.accent, SOFT_ALPHA[2]), under)
    }

    fn accent_text(&self, fills: &[Color]) -> Color {
        (0_u8..=10)
            .map(|step| mix(self.accent, self.text_primary, f32::from(step) / 10.0))
            .find(|&color| fills.iter().all(|&fill| readable(color, fill)))
            .unwrap_or(self.text_primary)
    }

    fn base_surfaces(&self, dark: bool, a: f32) -> [Color; 4] {
        let [black, white] =
            [Color::BLACK, Color::WHITE].map(|behind| composite(alpha(self.background, a), behind));
        [
            black,
            composite(selection(dark), black),
            white,
            composite(selection(dark), white),
        ]
    }

    // The most translucent tint that stays readable whether black or white is blurred behind.
    fn tint_alpha(&self, dark: bool) -> f32 {
        let glyphs = [self.accent, self.success, self.warning, self.error];
        (80_u8..=94)
            .map(|percent| f32::from(percent) / 100.0)
            .find(|&a| {
                let surfaces = self.base_surfaces(dark, a);
                readable(self.text_tertiary, surfaces[0])
                    && readable(self.text_tertiary, surfaces[2])
                    && surfaces.into_iter().all(|surface| {
                        readable(self.text_primary, surface)
                            && readable(self.text_secondary, surface)
                            && readable(self.text_primary, self.soft(surface))
                            && glyphs
                                .into_iter()
                                .all(|glyph| contrast_ratio(glyph, surface) >= MIN_CONTROL_CONTRAST)
                    })
            })
            .unwrap_or(1.0)
    }
}

impl TryFrom<RawColors> for Colors {
    type Error = ThemeError;

    fn try_from(raw: RawColors) -> Result<Self, Self::Error> {
        let hex = |field: &'static str, value: &str| {
            parse_hex(value).ok_or_else(|| ThemeError::InvalidColor {
                field,
                value: value.to_owned(),
            })
        };
        Ok(Self {
            background: hex("background", &raw.background)?,
            text_primary: hex("text_primary", &raw.text_primary)?,
            text_secondary: hex("text_secondary", &raw.text_secondary)?,
            text_tertiary: hex("text_tertiary", &raw.text_tertiary)?,
            accent: hex("accent", &raw.accent)?,
            success: hex("success", &raw.success)?,
            warning: hex("warning", &raw.warning)?,
            error: hex("error", &raw.error)?,
            border: hex("border", &raw.border)?,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTheme {
    id: String,
    name: Option<String>,
    appearance: Appearance,
    colors: RawColors,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawColors {
    background: String,
    text_primary: String,
    text_secondary: String,
    text_tertiary: String,
    accent: String,
    success: String,
    warning: String,
    error: String,
    border: String,
}

fn read_user_theme(path: &Path) -> std::io::Result<String> {
    let file = std::fs::File::open(path)?;
    let mut content = String::new();
    file.take(MAX_USER_THEME_BYTES + 1)
        .read_to_string(&mut content)?;
    if content.len() as u64 > MAX_USER_THEME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "theme file is too large",
        ));
    }
    Ok(content)
}

fn parse_hex(hex: &str) -> Option<Color> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(Color::from_rgb8(channel(0)?, channel(2)?, channel(4)?))
}

fn composite(foreground: Color, background: Color) -> Color {
    let a = foreground.a;
    Color::from_rgb(
        foreground.r.mul_add(a, background.r * (1.0 - a)),
        foreground.g.mul_add(a, background.g * (1.0 - a)),
        foreground.b.mul_add(a, background.b * (1.0 - a)),
    )
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::from_rgb(
        (b.r - a.r).mul_add(t, a.r),
        (b.g - a.g).mul_add(t, a.g),
        (b.b - a.b).mul_add(t, a.b),
    )
}

fn contrast_ratio(a: Color, b: Color) -> f32 {
    let (a, b) = (relative_luminance(a), relative_luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn relative_luminance(color: Color) -> f32 {
    let linear = |c: f32| {
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.0722f32.mul_add(
        linear(color.b),
        0.2126f32.mul_add(linear(color.r), 0.7152 * linear(color.g)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEME: &str = r##"
id = "custom"
appearance = "dark"

[colors]
background = "#101010"
text_primary = "#FFFFFF"
text_secondary = "#C8C8C8"
text_tertiary = "#A8A8A8"
accent = "#8080F0"
success = "#3CCB7F"
warning = "#F5A623"
error = "#F27272"
border = "#FFFFFF"
"##;

    const NOCTALIA_LIGHT: &str = r##"
id = "noctalia"
appearance = "light"

[colors]
background = "#fdf8f5"
text_primary = "#1c1b1a"
text_secondary = "#49473e"
text_tertiary = "#7a776d"
accent = "#635e4b"
success = "#635e4b"
warning = "#615e55"
error = "#ba1a1a"
border = "#7a776d"
"##;

    #[test]
    fn built_in_themes_need_no_repair_and_stay_translucent() {
        for content in BUILT_IN_THEMES {
            let raw: RawTheme = toml::from_str(content).expect("built-in theme parses");
            let dark = raw.appearance == Appearance::Dark;
            let colors = Colors::try_from(raw.colors).expect("built-in colours parse");
            let failure = colors.validate(dark).err();
            assert!(
                failure.is_none(),
                "{}: {:?} needs repair",
                raw.id,
                failure.map(|f| (f.pair, f.ratio))
            );
            let tint = colors.tint_alpha(dark);
            assert!(
                (0.80..=0.94).contains(&tint),
                "{}: blur tint {tint} out of range",
                raw.id
            );
        }
    }

    #[test]
    fn unknown_theme_falls_back_to_dark() {
        let catalog = ThemeCatalog::built_in();
        let missing = ThemeId::parse("missing").expect("valid id");
        assert_eq!(catalog.resolve(&missing, false).id, ThemeId::default());
    }

    #[test]
    fn low_contrast_themes_are_repaired() {
        let cases = [
            (
                "text_secondary = \"#C8C8C8\"",
                "text_secondary = \"#333333\"",
            ),
            ("accent = \"#8080F0\"", "accent = \"#3A3AA0\""),
            ("error = \"#F27272\"", "error = \"#B03030\""),
        ];
        for (from, to) in cases {
            let content = THEME.replacen(from, to, 1);
            assert!(ThemeDefinition::parse(&content).is_ok(), "{to}");
        }
        assert!(ThemeDefinition::parse(NOCTALIA_LIGHT).is_ok());
    }

    #[test]
    fn hostile_or_unreadable_themes_are_rejected() {
        type Expect = fn(&ThemeError) -> bool;
        let cases: [(&str, &str, Expect); 5] = [
            ("border = \"#FFFFFF\"", "", |e| {
                matches!(e, ThemeError::Parse(_))
            }),
            (
                "border = \"#FFFFFF\"",
                "border = \"#FFFFFF\"\nsurface = \"#151515\"",
                |e| matches!(e, ThemeError::Parse(_)),
            ),
            ("accent = \"#8080F0\"", "accent = \"8080F0\"", |e| {
                matches!(
                    e,
                    ThemeError::InvalidColor {
                        field: "accent",
                        ..
                    }
                )
            }),
            ("id = \"custom\"", "id = \"Custom Theme\"", |e| {
                matches!(e, ThemeError::InvalidId(_))
            }),
            (
                "background = \"#101010\"",
                "background = \"#777777\"",
                |e| matches!(e, ThemeError::LowContrast { .. }),
            ),
        ];
        assert!(ThemeDefinition::parse(THEME).is_ok());
        for (from, to, expected) in cases {
            let content = THEME.replacen(from, to, 1);
            let error = ThemeDefinition::parse(&content).expect_err(to);
            assert!(expected(&error), "{to}: unexpected {error}");
        }
    }
}
