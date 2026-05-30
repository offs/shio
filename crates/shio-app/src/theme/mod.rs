use crate::style::Palette;
use iced::Color;
use serde::Deserialize;
use shio_core::ThemeConfig;
use std::collections::HashSet;
use std::path::Path;

const BUILT_IN_THEMES: &[&str] = &[
    include_str!("../../../../assets/themes/dark.toml"),
    include_str!("../../../../assets/themes/light.toml"),
    include_str!("../../../../assets/themes/zen.toml"),
    include_str!("../../../../assets/themes/catppuccin.toml"),
    include_str!("../../../../assets/themes/rose-pine.toml"),
    include_str!("../../../../assets/themes/github-dark.toml"),
    include_str!("../../../../assets/themes/dawn.toml"),
];
const MIN_TEXT_CONTRAST: f32 = 4.5;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ThemeId(String);

impl ThemeId {
    pub(crate) fn parse(value: &str) -> Result<Self, ThemeError> {
        shio_core::validate_theme_id(value)
            .map_err(|_| ThemeError::InvalidId(value.to_string()))?;
        Ok(Self(value.to_string()))
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
pub(crate) enum ThemeAppearance {
    Light,
    Dark,
}

#[derive(Debug, Clone)]
pub(crate) struct ThemeDefinition {
    pub(crate) id: ThemeId,
    pub(crate) appearance: ThemeAppearance,
    colors: ThemeColors,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ThemeColors {
    background: Color,
    surface: Color,
    elevated: Color,
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
pub(crate) struct ThemeSelection {
    pub(crate) id: ThemeId,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedTheme {
    pub(crate) id: ThemeId,
    pub(crate) requested_id: ThemeId,
    pub(crate) used_fallback: bool,
    pub(crate) palette: Palette,
    pub(crate) iced_theme: iced::Theme,
}

#[derive(Debug)]
pub(crate) enum ThemeError {
    Parse(toml::de::Error),
    InvalidId(String),
    InvalidColor { field: &'static str, value: String },
    DuplicateId(String),
    LowContrast(String),
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "parse error: {e}"),
            Self::InvalidId(id) => write!(f, "invalid id: {id}"),
            Self::InvalidColor { field, value } => write!(f, "invalid color {field}: {value}"),
            Self::DuplicateId(id) => write!(f, "duplicate id: {id}"),
            Self::LowContrast(id) => write!(f, "theme has low required contrast: {id}"),
        }
    }
}

impl ThemeCatalog {
    pub(crate) fn load(user_theme_dir: &Path) -> Self {
        let mut catalog = Self::built_in();
        catalog.load_user_themes(user_theme_dir);
        catalog
    }

    pub(crate) fn built_in() -> Self {
        let mut themes = Vec::with_capacity(BUILT_IN_THEMES.len());
        let mut ids = HashSet::new();

        for content in BUILT_IN_THEMES {
            let definition = match ThemeDefinition::parse(content, true) {
                Ok(definition) => definition,
                Err(e) => panic!("built-in theme must validate: {e}"),
            };
            let inserted = ids.insert(definition.id.clone());
            assert!(inserted, "duplicate built-in theme id: {}", definition.id);
            themes.push(definition);
        }

        Self { themes }
    }

    pub(crate) fn resolve(&self, selection: &ThemeSelection) -> ResolvedTheme {
        let requested_id = selection.id.clone();
        let definition = if let Some(definition) = self.find(&selection.id) {
            definition
        } else {
            tracing::warn!("theme '{}' not found; using dark", selection.id);
            self.find(static_theme_id("dark"))
                .unwrap_or_else(|| self.first())
        };
        let mut resolved = definition.resolve();
        resolved.requested_id = requested_id;
        resolved.used_fallback = resolved.id != resolved.requested_id;
        resolved
    }

    pub(crate) fn ids(&self) -> Vec<ThemeId> {
        self.themes
            .iter()
            .map(|definition| definition.id.clone())
            .collect()
    }

    fn load_user_themes(&mut self, dir: &Path) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                tracing::warn!("failed to read theme directory {}: {e}", dir.display());
                return;
            },
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    tracing::warn!("failed to read theme directory entry: {e}");
                    continue;
                },
            };
            let path = entry.path();
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("toml") {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(e) => {
                    tracing::warn!("failed to read theme {}: {e}", path.display());
                    continue;
                },
            };
            let definition = match ThemeDefinition::parse(&content, false) {
                Ok(definition) => definition,
                Err(e) => {
                    tracing::warn!("invalid theme {}: {e}", path.display());
                    continue;
                },
            };
            if let Err(e) = self.add_user_theme(definition) {
                tracing::warn!("invalid theme {}: {e}", path.display());
            }
        }
    }

    fn add_user_theme(&mut self, definition: ThemeDefinition) -> Result<(), ThemeError> {
        if self.themes.iter().any(|theme| theme.id == definition.id) {
            return Err(ThemeError::DuplicateId(definition.id.to_string()));
        }
        self.themes.push(definition);
        Ok(())
    }

    fn find(&self, id: &ThemeId) -> Option<&ThemeDefinition> {
        self.themes.iter().find(|definition| definition.id == *id)
    }

    fn first(&self) -> &ThemeDefinition {
        self.themes
            .first()
            .unwrap_or_else(|| panic!("built-in themes must not be empty"))
    }
}

impl ThemeSelection {
    pub(crate) fn from_config(config: &ThemeConfig) -> Self {
        let id = ThemeId::parse(&config.id).unwrap_or_else(|_| static_theme_id("dark").clone());
        Self { id }
    }
}

impl ThemeDefinition {
    pub(crate) fn parse(content: &str, _built_in: bool) -> Result<Self, ThemeError> {
        let raw: RawThemeDefinition = toml::from_str(content).map_err(ThemeError::Parse)?;
        let definition = Self {
            id: ThemeId::parse(&raw.id)?,
            appearance: raw.appearance,
            colors: ThemeColors::try_from(raw.colors)?,
        };
        definition.validate_contrast()?;
        Ok(definition)
    }

    fn resolve(&self) -> ResolvedTheme {
        let palette = match self.id.as_str() {
            "dark" => dark_palette(),
            "light" => light_palette(),
            _ => self.colors.to_palette(self.appearance),
        };
        let iced_theme = iced::Theme::custom(
            self.id.to_string(),
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
            id: self.id.clone(),
            requested_id: self.id.clone(),
            used_fallback: false,
            palette,
            iced_theme,
        }
    }

    fn validate_contrast(&self) -> Result<(), ThemeError> {
        if contrast_ratio(self.colors.text_primary, self.colors.background) < MIN_TEXT_CONTRAST {
            return Err(ThemeError::LowContrast(self.id.to_string()));
        }
        let acrylic = acrylic_composite_probe(self.colors.background, self.appearance);
        if contrast_ratio(self.colors.text_primary, acrylic) < MIN_TEXT_CONTRAST {
            return Err(ThemeError::LowContrast(self.id.to_string()));
        }
        Ok(())
    }
}

impl ThemeColors {
    fn to_palette(self, appearance: ThemeAppearance) -> Palette {
        let dark = appearance == ThemeAppearance::Dark;
        let hover = overlay(dark, if dark { 0.08 } else { 0.05 });
        let subtle = overlay(dark, 0.06);
        let default_border = alpha(self.border, if dark { 0.08 } else { 0.09 });
        let scroller = overlay(dark, 0.12);
        let scroller_hovered = overlay(dark, 0.22);
        let scroller_dragged = overlay(dark, 0.32);

        Palette {
            bg_base: alpha(self.background, if dark { 0.92 } else { 0.95 }),
            bg_surface: alpha(self.surface, if dark { 0.70 } else { 0.85 }),
            bg_elevated: alpha(self.elevated, if dark { 0.97 } else { 1.0 }),
            bg_hover: hover,
            bg_active: if dark {
                lighten(self.surface, 0.08)
            } else {
                darken(self.surface, 0.04)
            },
            text_primary: self.text_primary,
            text_secondary: self.text_secondary,
            text_tertiary: self.text_tertiary,
            text_ghost: alpha(self.text_tertiary, if dark { 0.68 } else { 0.72 }),
            accent: self.accent,
            success: self.success,
            warning: self.warning,
            error: self.error,
            border_subtle: alpha(self.border, 0.06),
            border_default: default_border,
            overlay_hover: overlay(dark, if dark { 0.06 } else { 0.04 }),
            overlay_subtle: subtle,
            scroller_idle: scroller,
            scroller_hovered,
            scroller_dragged,
            progress_bg: subtle,
            toggler_off_bg: overlay(dark, 0.10),
            toggler_off_fg: alpha(self.text_tertiary, if dark { 0.70 } else { 0.80 }),
        }
    }
}

impl TryFrom<RawThemeColors> for ThemeColors {
    type Error = ThemeError;

    fn try_from(raw: RawThemeColors) -> Result<Self, Self::Error> {
        Ok(Self {
            background: parse_required_hex("background", &raw.background)?,
            surface: parse_required_hex("surface", &raw.surface)?,
            elevated: parse_required_hex("elevated", &raw.elevated)?,
            text_primary: parse_required_hex("text_primary", &raw.text_primary)?,
            text_secondary: parse_required_hex("text_secondary", &raw.text_secondary)?,
            text_tertiary: parse_required_hex("text_tertiary", &raw.text_tertiary)?,
            accent: parse_required_hex("accent", &raw.accent)?,
            success: parse_required_hex("success", &raw.success)?,
            warning: parse_required_hex("warning", &raw.warning)?,
            error: parse_required_hex("error", &raw.error)?,
            border: parse_required_hex("border", &raw.border)?,
        })
    }
}

impl std::fmt::Display for ThemeAppearance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Light => "light",
            Self::Dark => "dark",
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawThemeDefinition {
    id: String,
    appearance: ThemeAppearance,
    colors: RawThemeColors,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawThemeColors {
    background: String,
    surface: String,
    elevated: String,
    text_primary: String,
    text_secondary: String,
    text_tertiary: String,
    accent: String,
    success: String,
    warning: String,
    error: String,
    border: String,
}

fn static_theme_id(value: &'static str) -> &'static ThemeId {
    use std::sync::OnceLock;

    static DARK: OnceLock<ThemeId> = OnceLock::new();
    static LIGHT: OnceLock<ThemeId> = OnceLock::new();

    match value {
        "dark" => DARK.get_or_init(|| ThemeId("dark".to_string())),
        "light" => LIGHT.get_or_init(|| ThemeId("light".to_string())),
        _ => panic!("unsupported static theme id: {value}"),
    }
}

fn parse_required_hex(field: &'static str, value: &str) -> Result<Color, ThemeError> {
    parse_hex(value).ok_or_else(|| ThemeError::InvalidColor {
        field,
        value: value.to_string(),
    })
}

pub(crate) fn parse_hex(hex: &str) -> Option<Color> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::from_rgb8(r, g, b))
}

const fn alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

const fn overlay(dark: bool, a: f32) -> Color {
    if dark {
        Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a,
        }
    } else {
        Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a,
        }
    }
}

fn lighten(color: Color, amount: f32) -> Color {
    Color {
        r: (1.0 - color.r).mul_add(amount, color.r),
        g: (1.0 - color.g).mul_add(amount, color.g),
        b: (1.0 - color.b).mul_add(amount, color.b),
        a: color.a,
    }
}

fn darken(color: Color, amount: f32) -> Color {
    Color {
        r: color.r * (1.0 - amount),
        g: color.g * (1.0 - amount),
        b: color.b * (1.0 - amount),
        a: color.a,
    }
}

fn acrylic_composite_probe(background: Color, appearance: ThemeAppearance) -> Color {
    let blur = match appearance {
        ThemeAppearance::Dark => Color::from_rgb8(32, 33, 36),
        ThemeAppearance::Light => Color::from_rgb8(244, 244, 244),
    };
    composite(alpha(background, 240.0 / 255.0), blur)
}

fn composite(foreground: Color, background: Color) -> Color {
    let inverse_alpha = 1.0 - foreground.a;
    let a = background.a.mul_add(inverse_alpha, foreground.a);
    if a == 0.0 {
        return Color::TRANSPARENT;
    }
    Color {
        r: foreground
            .r
            .mul_add(foreground.a, background.r * background.a * inverse_alpha)
            / a,
        g: foreground
            .g
            .mul_add(foreground.a, background.g * background.a * inverse_alpha)
            / a,
        b: foreground
            .b
            .mul_add(foreground.a, background.b * background.a * inverse_alpha)
            / a,
        a,
    }
}

fn contrast_ratio(a: Color, b: Color) -> f32 {
    let light = relative_luminance(a).max(relative_luminance(b));
    let dark = relative_luminance(a).min(relative_luminance(b));
    (light + 0.05) / (dark + 0.05)
}

fn relative_luminance(color: Color) -> f32 {
    let red = linear_channel(color.r);
    let green = linear_channel(color.g);
    let blue = linear_channel(color.b);
    0.0722f32.mul_add(blue, 0.2126f32.mul_add(red, 0.7152 * green))
}

fn linear_channel(channel: f32) -> f32 {
    if channel <= 0.03928 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

const fn dark_palette() -> Palette {
    Palette {
        bg_base: Color {
            r: 0.03,
            g: 0.03,
            b: 0.035,
            a: 0.92,
        },
        bg_surface: Color {
            r: 0.06,
            g: 0.06,
            b: 0.07,
            a: 0.70,
        },
        bg_elevated: Color {
            r: 0.102,
            g: 0.102,
            b: 0.118,
            a: 0.97,
        },
        bg_hover: Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.08,
        },
        bg_active: Color {
            r: 0.149,
            g: 0.149,
            b: 0.180,
            a: 1.0,
        },
        text_primary: Color {
            r: 0.929,
            g: 0.929,
            b: 0.937,
            a: 1.0,
        },
        text_secondary: Color {
            r: 0.55,
            g: 0.55,
            b: 0.59,
            a: 1.0,
        },
        text_tertiary: Color {
            r: 0.35,
            g: 0.35,
            b: 0.38,
            a: 1.0,
        },
        text_ghost: Color {
            r: 0.231,
            g: 0.231,
            b: 0.267,
            a: 1.0,
        },
        accent: Color {
            r: 0.357,
            g: 0.357,
            b: 0.839,
            a: 1.0,
        },
        success: Color {
            r: 0.235,
            g: 0.796,
            b: 0.498,
            a: 1.0,
        },
        warning: Color {
            r: 0.961,
            g: 0.651,
            b: 0.137,
            a: 1.0,
        },
        error: Color {
            r: 0.863,
            g: 0.298,
            b: 0.298,
            a: 1.0,
        },
        border_subtle: Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.06,
        },
        border_default: Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.08,
        },
        overlay_hover: Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.06,
        },
        overlay_subtle: Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.06,
        },
        scroller_idle: Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.12,
        },
        scroller_hovered: Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.22,
        },
        scroller_dragged: Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.32,
        },
        progress_bg: Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.06,
        },
        toggler_off_bg: Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.10,
        },
        toggler_off_fg: Color {
            r: 0.231,
            g: 0.231,
            b: 0.267,
            a: 1.0,
        },
    }
}

const fn light_palette() -> Palette {
    Palette {
        bg_base: Color {
            r: 0.98,
            g: 0.98,
            b: 0.98,
            a: 0.95,
        },
        bg_surface: Color {
            r: 0.941,
            g: 0.941,
            b: 0.949,
            a: 0.85,
        },
        bg_elevated: Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        },
        bg_hover: Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.05,
        },
        bg_active: Color {
            r: 0.91,
            g: 0.91,
            b: 0.925,
            a: 1.0,
        },
        text_primary: Color {
            r: 0.102,
            g: 0.102,
            b: 0.118,
            a: 1.0,
        },
        text_secondary: Color {
            r: 0.396,
            g: 0.396,
            b: 0.427,
            a: 1.0,
        },
        text_tertiary: Color {
            r: 0.627,
            g: 0.627,
            b: 0.671,
            a: 1.0,
        },
        text_ghost: Color {
            r: 0.804,
            g: 0.804,
            b: 0.831,
            a: 1.0,
        },
        accent: Color {
            r: 0.357,
            g: 0.357,
            b: 0.839,
            a: 1.0,
        },
        success: Color {
            r: 0.176,
            g: 0.659,
            b: 0.400,
            a: 1.0,
        },
        warning: Color {
            r: 0.831,
            g: 0.565,
            b: 0.039,
            a: 1.0,
        },
        error: Color {
            r: 0.788,
            g: 0.235,
            b: 0.235,
            a: 1.0,
        },
        border_subtle: Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.06,
        },
        border_default: Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.09,
        },
        overlay_hover: Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.04,
        },
        overlay_subtle: Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.06,
        },
        scroller_idle: Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.12,
        },
        scroller_hovered: Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.22,
        },
        scroller_dragged: Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.32,
        },
        progress_bg: Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.06,
        },
        toggler_off_bg: Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.10,
        },
        toggler_off_fg: Color {
            r: 0.804,
            g: 0.804,
            b: 0.831,
            a: 1.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(id: &str) -> ThemeSelection {
        ThemeSelection {
            id: ThemeId::parse(id).expect("valid theme id"),
        }
    }

    #[test]
    fn built_in_dark_preserves_palette() {
        let catalog = ThemeCatalog::built_in();
        let resolved = catalog.resolve(&selection("dark"));

        assert_eq!(resolved.palette, dark_palette());
    }

    #[test]
    fn built_in_light_preserves_palette() {
        let catalog = ThemeCatalog::built_in();
        let resolved = catalog.resolve(&selection("light"));

        assert_eq!(resolved.palette, light_palette());
    }

    #[test]
    fn missing_selection_falls_back_to_dark() {
        let catalog = ThemeCatalog::built_in();
        let resolved = catalog.resolve(&selection("missing"));

        assert_eq!(resolved.id.as_str(), "dark");
        assert_eq!(resolved.requested_id.as_str(), "missing");
        assert!(resolved.used_fallback);
    }

    #[test]
    fn theme_resolution_does_not_require_material() {
        let catalog = ThemeCatalog::built_in();
        let resolved = catalog.resolve(&selection("zen"));

        assert_eq!(resolved.id.as_str(), "zen");
    }

    #[test]
    fn built_in_catalog_exposes_expected_ids() {
        let catalog = ThemeCatalog::built_in();
        let ids: Vec<String> = catalog.ids().into_iter().map(|id| id.to_string()).collect();

        assert_eq!(
            ids,
            vec![
                "dark",
                "light",
                "zen",
                "catppuccin",
                "rose-pine",
                "github-dark",
                "dawn",
            ]
        );
    }

    #[test]
    fn unknown_theme_fields_are_rejected() {
        let toml = r##"
id = "custom"
name = "Custom"
appearance = "dark"

[colors]
background = "#000000"
surface = "#101010"
elevated = "#181818"
text_primary = "#FFFFFF"
text_secondary = "#BBBBBB"
text_tertiary = "#888888"
accent = "#5B5BD6"
success = "#3CCB7F"
warning = "#F5A623"
error = "#DC4C4C"
border = "#FFFFFF"
"##;

        assert!(matches!(
            ThemeDefinition::parse(toml, false),
            Err(ThemeError::Parse(_))
        ));
    }

    #[test]
    fn invalid_hex_is_rejected() {
        let toml = r##"
id = "custom"
appearance = "dark"

[colors]
background = "#000000"
surface = "#101010"
elevated = "#181818"
text_primary = "#FFFFFF"
text_secondary = "#BBBBBB"
text_tertiary = "#888888"
accent = "5B5BD6"
success = "#3CCB7F"
warning = "#F5A623"
error = "#DC4C4C"
border = "#FFFFFF"
"##;

        assert!(matches!(
            ThemeDefinition::parse(toml, false),
            Err(ThemeError::InvalidColor {
                field: "accent",
                ..
            })
        ));
    }

    #[test]
    fn bad_id_is_rejected() {
        let toml = r##"
id = "GitHub Dark"
appearance = "dark"

[colors]
background = "#000000"
surface = "#101010"
elevated = "#181818"
text_primary = "#FFFFFF"
text_secondary = "#BBBBBB"
text_tertiary = "#888888"
accent = "#5B5BD6"
success = "#3CCB7F"
warning = "#F5A623"
error = "#DC4C4C"
border = "#FFFFFF"
"##;

        assert!(matches!(
            ThemeDefinition::parse(toml, false),
            Err(ThemeError::InvalidId(_))
        ));
    }

    #[test]
    fn low_contrast_is_rejected() {
        let toml = r##"
id = "flat"
appearance = "dark"

[colors]
background = "#111111"
surface = "#151515"
elevated = "#202020"
text_primary = "#222222"
text_secondary = "#BBBBBB"
text_tertiary = "#888888"
accent = "#5B5BD6"
success = "#3CCB7F"
warning = "#F5A623"
error = "#DC4C4C"
border = "#FFFFFF"
"##;

        assert!(matches!(
            ThemeDefinition::parse(toml, false),
            Err(ThemeError::LowContrast(_))
        ));
    }
}
