use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

static SUBSCRIBERS: Mutex<Vec<UnboundedSender<TrayEvent>>> = Mutex::new(Vec::new());
static READY: AtomicBool = AtomicBool::new(false);
static CAN_PAUSE: AtomicBool = AtomicBool::new(true);
static CAN_RESUME: AtomicBool = AtomicBool::new(true);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayEvent {
    Show,
    PauseAll,
    ResumeAll,
    Quit,
}

#[cfg(target_os = "linux")]
pub(crate) use desktop::install as init;
#[cfg(target_os = "windows")]
pub(crate) use platform_thread::spawn as init;
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub(crate) const fn init() {}

// macOS can only create the tray once iced's native event loop runs.
#[cfg(target_os = "macos")]
pub(crate) use desktop::install as activate;
#[cfg(not(target_os = "macos"))]
pub(crate) const fn activate() {}

// Windows leaves a stale icon behind unless it is removed before exit.
#[cfg(target_os = "windows")]
pub(crate) use platform_thread::stop as shutdown;
#[cfg(not(target_os = "windows"))]
pub(crate) const fn shutdown() {}

pub(crate) fn subscribe() -> UnboundedReceiver<TrayEvent> {
    let (tx, rx) = unbounded_channel();
    SUBSCRIBERS.lock().push(tx);
    rx
}

pub(crate) fn is_ready() -> bool {
    READY.load(Ordering::Acquire)
}

pub(crate) fn set_actions(can_pause: bool, can_resume: bool) {
    if store_actions(can_pause, can_resume) {
        #[cfg(target_os = "windows")]
        platform_thread::wake();
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        desktop::sync_actions();
    }
}

fn store_actions(can_pause: bool, can_resume: bool) -> bool {
    let pause = CAN_PAUSE.swap(can_pause, Ordering::AcqRel) != can_pause;
    let resume = CAN_RESUME.swap(can_resume, Ordering::AcqRel) != can_resume;
    pause || resume
}

fn dispatch(event: TrayEvent) {
    SUBSCRIBERS.lock().retain(|tx| tx.send(event).is_ok());
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
mod desktop {
    use super::{CAN_PAUSE, CAN_RESUME, TrayEvent, dispatch};
    use std::cell::RefCell;
    use std::sync::atomic::Ordering;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

    struct Tray {
        _icon: TrayIcon,
        pause: MenuItem,
        resume: MenuItem,
    }

    // The menu items are bound to the thread that created them.
    thread_local!(static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) });

    pub(crate) fn install() {
        if TRAY.with_borrow(Option::is_some) {
            return;
        }
        match build() {
            Ok(tray) => {
                TRAY.set(Some(tray));
                sync_actions();
                #[cfg(target_os = "linux")]
                super::host::watch();
                #[cfg(not(target_os = "linux"))]
                super::READY.store(true, Ordering::Release);
            },
            Err(error) => tracing::warn!("tray unavailable: {error}"),
        }
    }

    #[cfg(target_os = "windows")]
    pub(super) fn uninstall() {
        TRAY.set(None);
        super::READY.store(false, Ordering::Release);
    }

    pub(super) fn sync_actions() {
        TRAY.with_borrow(|tray| {
            if let Some(tray) = tray {
                tray.pause.set_enabled(CAN_PAUSE.load(Ordering::Acquire));
                tray.resume.set_enabled(CAN_RESUME.load(Ordering::Acquire));
            }
        });
    }

    fn build() -> Result<Tray, Box<dyn std::error::Error>> {
        let show = MenuItem::new("show shio", true, None);
        let pause = MenuItem::new("pause all", true, None);
        let resume = MenuItem::new("resume all", true, None);
        let quit = MenuItem::new("quit", true, None);
        let menu = Menu::with_items(&[
            &show,
            &pause,
            &resume,
            &PredefinedMenuItem::separator(),
            &quit,
        ])?;
        let events = [
            (show.id().clone(), TrayEvent::Show),
            (pause.id().clone(), TrayEvent::PauseAll),
            (resume.id().clone(), TrayEvent::ResumeAll),
            (quit.id().clone(), TrayEvent::Quit),
        ];
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some((_, event)) = events.iter().find(|(id, _)| *id == event.id) {
                dispatch(*event);
            }
        }));
        TrayIconEvent::set_event_handler(Some(|event| {
            // The StatusNotifierItem host reports its activate action as a left click.
            let show = if cfg!(target_os = "linux") {
                matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        ..
                    }
                )
            } else {
                matches!(event, TrayIconEvent::DoubleClick { .. })
            };
            if show {
                dispatch(TrayEvent::Show);
            }
        }));

        let image = image::load_from_memory(include_bytes!("../../../assets/icons/icon-64.png"))?
            .to_rgba8();
        let (width, height) = image.dimensions();
        let icon = TrayIconBuilder::new()
            .with_id("shio")
            .with_tooltip("shio")
            .with_icon(Icon::from_rgba(image.into_raw(), width, height)?)
            .with_menu(Box::new(menu))
            .build()?;
        Ok(Tray {
            _icon: icon,
            pause,
            resume,
        })
    }
}

#[cfg(target_os = "linux")]
mod host {
    use super::{READY, TrayEvent, dispatch};
    use std::sync::atomic::Ordering;
    use zbus::blocking::{Connection, fdo::DBusProxy};

    const WATCHER: &str = "org.kde.StatusNotifierWatcher";

    pub(super) fn watch() {
        let spawned = std::thread::Builder::new()
            .name("shio-tray".into())
            .spawn(|| {
                if let Err(error) = run() {
                    tracing::warn!("no tray host watcher: {error}");
                }
            });
        if let Err(error) = spawned {
            tracing::warn!("tray thread failed to spawn: {error}");
        }
    }

    // GNOME without the AppIndicator extension has no watcher, and the window must never
    // hide into a tray nobody shows. Watchers always claim a registered host, so the
    // watcher's presence is the only signal.
    fn run() -> zbus::Result<()> {
        let bus = DBusProxy::new(&Connection::session()?)?;
        let changes = bus.receive_name_owner_changed_with_args(&[(0, WATCHER)])?;
        set_ready(bus.name_has_owner(WATCHER.try_into()?)?);
        for change in changes {
            set_ready(change.args()?.new_owner().is_some());
        }
        Ok(())
    }

    fn set_ready(ready: bool) {
        if READY.swap(ready, Ordering::AcqRel) && !ready {
            tracing::warn!("tray host disappeared; showing the window");
            dispatch(TrayEvent::Show);
        }
    }
}

#[cfg(target_os = "windows")]
mod platform_thread {
    #![allow(unsafe_code)]

    use super::desktop;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::thread::JoinHandle;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, PostThreadMessageW, TranslateMessage, WM_APP, WM_QUIT,
    };

    static THREAD_ID: AtomicU32 = AtomicU32::new(0);
    static THREAD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

    pub(crate) fn spawn() {
        match std::thread::Builder::new()
            .name("shio-tray".into())
            .spawn(run)
        {
            Ok(handle) => *THREAD.lock() = Some(handle),
            Err(error) => tracing::warn!("tray thread failed to spawn: {error}"),
        }
    }

    pub(super) fn wake() {
        post(WM_APP);
    }

    pub(crate) fn stop() {
        if post(WM_QUIT)
            && let Some(handle) = THREAD.lock().take()
        {
            let _ = handle.join();
        }
    }

    fn post(message: u32) -> bool {
        let id = THREAD_ID.load(Ordering::Acquire);
        // SAFETY: posting to a thread id has no memory-safety preconditions; a stale id fails.
        id != 0 && unsafe { PostThreadMessageW(id, message, 0, 0) } != 0
    }

    fn run() {
        // SAFETY: GetCurrentThreadId has no preconditions.
        THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
        desktop::install();
        // SAFETY: MSG is plain data and all-zero is a valid value.
        let mut message: MSG = unsafe { std::mem::zeroed() };
        // SAFETY: `message` is a valid out pointer for the duration of each call.
        while unsafe { GetMessageW(&raw mut message, std::ptr::null_mut(), 0, 0) } > 0 {
            if message.message == WM_APP {
                desktop::sync_actions();
            }
            // SAFETY: `message` was just filled by GetMessageW.
            unsafe {
                TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
        }
        desktop::uninstall();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_changes_are_detected_per_item() {
        store_actions(true, true);
        assert!(
            store_actions(true, false),
            "a resume-only change must resync"
        );
        assert!(!store_actions(true, false));
        assert!(store_actions(false, false));
    }
}
