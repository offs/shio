#![allow(unsafe_code)]

use iced::window::Window;
use iced::window::raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::cell::RefCell;
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::{
    self, Capability, ExtBackgroundEffectManagerV1,
};
use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

enum Support {
    Ext(Box<Ext>),
    Kde,
    None,
}

struct Ext {
    conn: Connection,
    queue: EventQueue<State>,
    compositor: WlCompositor,
    manager: ExtBackgroundEffectManagerV1,
    effect: Option<(WlSurface, ExtBackgroundEffectSurfaceV1)>,
}

#[derive(Default)]
struct State {
    can_blur: bool,
}

// `window::run` closures execute on winit's event-loop thread, so one cache per thread is one per app.
thread_local!(static SUPPORT: RefCell<Option<Support>> = const { RefCell::new(None) });

pub(super) fn set_blur(w: &dyn Window, on: bool, created: bool) -> bool {
    SUPPORT.with_borrow_mut(|support| {
        match support.get_or_insert_with(|| probe(w).unwrap_or(Support::None)) {
            Support::Ext(ext) => ext.set(w, on).is_some() && on,
            Support::Kde => on && created,
            Support::None => false,
        }
    })
}

/// winit's `focus_window` is a no-op on Wayland; raising needs the launcher's activation token.
pub(super) fn activate(w: &dyn Window, token: String) -> Option<()> {
    let conn = connection(w)?;
    let (globals, queue) = registry_queue_init::<State>(&conn).ok()?;
    let activation = globals
        .bind::<XdgActivationV1, _, _>(&queue.handle(), 1..=1, ())
        .ok()?;
    activation.activate(token, &surface(&conn, w)?);
    activation.destroy();
    conn.flush().ok()
}

impl Ext {
    fn set(&mut self, w: &dyn Window, on: bool) -> Option<()> {
        let surface = surface(&self.conn, w)?;
        if let Some((_, stale)) = self.effect.take_if(|(current, _)| *current != surface) {
            stale.destroy();
        }
        let (_, effect) = self.effect.get_or_insert_with(|| {
            let effect = self
                .manager
                .get_background_effect(&surface, &self.queue.handle(), ());
            (surface, effect)
        });
        // The region is clipped to the surface, so one unbounded rect survives every resize.
        let region = on.then(|| {
            let region = self.compositor.create_region(&self.queue.handle(), ());
            region.add(0, 0, i32::MAX, i32::MAX);
            region
        });
        effect.set_blur_region(region.as_ref());
        if let Some(region) = region {
            region.destroy();
        }
        self.conn.flush().ok()
    }
}

fn connection(w: &dyn Window) -> Option<Connection> {
    let RawDisplayHandle::Wayland(display) = w.display_handle().ok()?.as_raw() else {
        return None;
    };
    // SAFETY: the pointer is winit's live `wl_display`, which outlives every window and the
    // connections made from it here (all end with the event loop).
    let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
    Some(Connection::from_backend(backend))
}

fn probe(w: &dyn Window) -> Option<Support> {
    let conn = connection(w)?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
    let qh = queue.handle();
    let Ok(manager) = globals.bind::<ExtBackgroundEffectManagerV1, _, _>(&qh, 1..=1, ()) else {
        let kde = globals.contents().with_list(|list| {
            list.iter()
                .any(|g| g.interface == "org_kde_kwin_blur_manager")
        });
        return kde.then_some(Support::Kde);
    };
    let compositor = globals.bind::<WlCompositor, _, _>(&qh, 1..=1, ()).ok()?;
    let mut state = State::default();
    queue.roundtrip(&mut state).ok()?;
    state.can_blur.then(|| {
        Support::Ext(Box::new(Ext {
            conn,
            queue,
            compositor,
            manager,
            effect: None,
        }))
    })
}

fn surface(conn: &Connection, w: &dyn Window) -> Option<WlSurface> {
    let RawWindowHandle::Wayland(handle) = w.window_handle().ok()?.as_raw() else {
        return None;
    };
    // SAFETY: the pointer is the live `wl_surface` winit created for this window; `from_ptr`
    // checks its interface before wrapping it.
    let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), handle.surface.as_ptr().cast()) };
    WlSurface::from_id(conn, id.ok()?).ok()
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtBackgroundEffectManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtBackgroundEffectManagerV1,
        event: ext_background_effect_manager_v1::Event,
        &(): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_background_effect_manager_v1::Event::Capabilities {
            flags: WEnum::Value(flags),
        } = event
        {
            state.can_blur = flags.contains(Capability::Blur);
        }
    }
}

delegate_noop!(State: WlCompositor);
delegate_noop!(State: WlRegion);
delegate_noop!(State: ExtBackgroundEffectSurfaceV1);
delegate_noop!(State: XdgActivationV1);
