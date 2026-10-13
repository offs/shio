use crate::theme::{ThemeCatalog, ThemeId};
use shio_core::AppConfig;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const TEMPLATE: &str = include_str!("../../../../assets/noctalia/shio.toml");
// Noctalia re-renders every template (and runs their hooks) on a config change, which takes
// several seconds on a full setup.
const RENDER_TIMEOUT: Duration = Duration::from_secs(20);
const RENDER_POLL: Duration = Duration::from_millis(100);

/// Noctalia creates its config folder on startup, so its presence marks a noctalia desktop.
pub(crate) fn dir() -> Option<PathBuf> {
    let dir = AppConfig::config_dir().parent()?.join("noctalia");
    dir.is_dir().then_some(dir)
}

/// Noctalia merges and watches every `*.toml` in its folder, so the template must exist before
/// the fragment that names it.
pub(crate) fn follow(dir: &Path) -> Result<(ThemeCatalog, ThemeId), String> {
    let theme_dir = AppConfig::theme_dir();
    let output = toml::Value::String(theme_dir.join("noctalia.toml").to_string_lossy().into());
    let fragment = format!(
        "# written by shio so its noctalia theme follows your palette; delete to stop\n\
         [theme.templates.user.shio]\n\
         input_path = \"templates/shio.toml\"\n\
         output_path = {output}\n"
    );
    std::fs::create_dir_all(dir.join("templates"))
        .and_then(|()| std::fs::write(dir.join("templates/shio.toml"), TEMPLATE))
        .and_then(|()| std::fs::write(dir.join("shio.toml"), fragment))
        .map_err(|error| format!("could not set up noctalia: {error}"))?;

    let id = ThemeId::parse("noctalia").expect("noctalia is a valid theme id");
    let deadline = Instant::now() + RENDER_TIMEOUT;
    while Instant::now() < deadline {
        let catalog = ThemeCatalog::load(&theme_dir);
        if catalog.find(&id).is_some() {
            return Ok((catalog, id));
        }
        std::thread::sleep(RENDER_POLL);
    }
    Err("noctalia did not render shio's theme — is it running?".to_owned())
}
