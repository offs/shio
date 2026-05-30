use iced::Color;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Palette {
    pub bg_base: Color,
    pub bg_surface: Color,
    pub bg_elevated: Color,
    pub bg_hover: Color,
    pub bg_active: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_tertiary: Color,
    pub text_ghost: Color,
    pub accent: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub border_subtle: Color,
    pub border_default: Color,
    pub overlay_hover: Color,
    pub overlay_subtle: Color,
    pub scroller_idle: Color,
    pub scroller_hovered: Color,
    pub scroller_dragged: Color,
    pub progress_bg: Color,
    pub toggler_off_bg: Color,
    pub toggler_off_fg: Color,
}
