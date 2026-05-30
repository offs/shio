#[cfg(not(target_os = "windows"))]
use std::sync::mpsc::Receiver;

#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) enum TrayEvent {
    Show,
    PauseAll,
    ResumeAll,
    Quit,
}

#[cfg(target_os = "windows")]
pub(crate) use windows::{init, subscribe};

#[cfg(not(target_os = "windows"))]
#[allow(clippy::missing_const_for_fn)]
pub(crate) fn init() {}

#[cfg(not(target_os = "windows"))]
#[allow(clippy::missing_const_for_fn)]
pub(crate) fn subscribe() -> Option<Receiver<TrayEvent>> {
    None
}

#[cfg(target_os = "windows")]
mod windows {
    use super::TrayEvent;
    use parking_lot::Mutex;
    use std::sync::OnceLock;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::thread;
    use tray_icon::Icon;
    use tray_icon::TrayIconBuilder;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};

    const ICON_BYTES: &[u8] = include_bytes!("../../../assets/icons/icon-32.png");

    static SUBSCRIBERS: OnceLock<Mutex<Vec<Sender<TrayEvent>>>> = OnceLock::new();

    pub(crate) fn init() {
        let _ = SUBSCRIBERS.set(Mutex::new(Vec::new()));
        if let Err(e) = spawn_thread() {
            tracing::warn!("tray thread failed to spawn: {e}");
        }
    }

    pub(crate) fn subscribe() -> Option<Receiver<TrayEvent>> {
        let (tx, rx) = channel::<TrayEvent>();
        SUBSCRIBERS.get()?.lock().push(tx);
        Some(rx)
    }

    fn dispatch(event: TrayEvent) {
        let Some(subscribers) = SUBSCRIBERS.get() else {
            return;
        };
        let mut subscribers = subscribers.lock();
        subscribers.retain(|tx| tx.send(event).is_ok());
        if subscribers.is_empty() {
            tracing::debug!("tray event dropped without subscribers");
        }
    }

    fn spawn_thread() -> std::io::Result<()> {
        thread::Builder::new()
            .name("shio-tray".into())
            .spawn(run)
            .map(|_| ())
    }

    fn run() {
        let Some(icon) = load_icon() else {
            tracing::warn!("failed to load tray icon");
            return;
        };

        let menu = Menu::new();
        let show = MenuItem::new("show shio", true, None);
        let pause = MenuItem::new("pause all", true, None);
        let resume = MenuItem::new("resume all", true, None);
        let sep = PredefinedMenuItem::separator();
        let quit = MenuItem::new("quit", true, None);
        for item in [&show, &pause, &resume] {
            if let Err(error) = menu.append(item) {
                tracing::warn!("tray menu item failed: {error}");
            }
        }
        if let Err(error) = menu.append(&sep) {
            tracing::warn!("tray menu separator failed: {error}");
        }
        if let Err(error) = menu.append(&quit) {
            tracing::warn!("tray menu item failed: {error}");
        }

        let show_id = show.id().clone();
        let pause_id = pause.id().clone();
        let resume_id = resume.id().clone();
        let quit_id = quit.id().clone();

        let _tray = match TrayIconBuilder::new()
            .with_tooltip("shio")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .build()
        {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("tray build failed: {e}");
                return;
            },
        };

        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let id = &event.id;
            let ev = if id == &show_id {
                TrayEvent::Show
            } else if id == &pause_id {
                TrayEvent::PauseAll
            } else if id == &resume_id {
                TrayEvent::ResumeAll
            } else if id == &quit_id {
                TrayEvent::Quit
            } else {
                return;
            };
            dispatch(ev);
        }));

        tray_icon::TrayIconEvent::set_event_handler(Some(|event: tray_icon::TrayIconEvent| {
            if let tray_icon::TrayIconEvent::DoubleClick { .. } = event {
                dispatch(TrayEvent::Show);
            }
        }));

        pump_messages();
    }

    fn load_icon() -> Option<Icon> {
        let img = image::load_from_memory(ICON_BYTES).ok()?.to_rgba8();
        let (w, h) = img.dimensions();
        Icon::from_rgba(img.into_raw(), w, h).ok()
    }

    fn pump_messages() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, GetMessageW, MSG, TranslateMessage,
        };
        #[allow(unsafe_code)]
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            loop {
                let ret = GetMessageW(&raw mut msg, std::ptr::null_mut(), 0, 0);
                if ret <= 0 {
                    break;
                }
                let _ = TranslateMessage(&raw const msg);
                DispatchMessageW(&raw const msg);
            }
        }
    }
}
