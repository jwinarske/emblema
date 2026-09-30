//! EGL window-surface presentation for Wayland, X11, and Android.

pub mod window;

pub use window::WindowTarget;

/// Create an offscreen surface with no window behind it.
///
/// The EGL counterpart of a headless Vulkan surface: everything a window
/// surface does — being made current, being blitted into, being swapped —
/// happens the same way, and nothing is displayed. What it cannot show is what
/// a window system would put on screen, which is why the presentation target
/// states its orientation as a relationship between two images rather than as
/// an appearance.
///
/// Offered here rather than confined to a test for the same reason the Vulkan
/// one is: a conformance run, a soak test, and a developer on a headless box
/// all want it, and each would otherwise write it themselves.
///
/// The surface is the caller's to destroy, with [`destroy_surface`], after the
/// target built on it has been destroyed.
pub fn create_offscreen_surface(
    ctx: &emblema_hal_gles::GlesContext,
    extent: emblema_hal::Extent2D,
) -> emblema_hal::Result<khronos_egl::Surface> {
    let (egl, display, _, config) = ctx.egl();
    let attributes = [
        khronos_egl::WIDTH,
        extent.width as i32,
        khronos_egl::HEIGHT,
        extent.height as i32,
        khronos_egl::NONE,
    ];
    egl.create_pbuffer_surface(display, config, &attributes)
        .map_err(|e| emblema_hal::Error::Backend {
            backend: "gles",
            detail: format!("create_pbuffer_surface: {e:?}"),
        })
}

/// Release a surface created by [`create_offscreen_surface`].
pub fn destroy_surface(
    ctx: &emblema_hal_gles::GlesContext,
    surface: khronos_egl::Surface,
) -> emblema_hal::Result<()> {
    let (egl, display, _, _) = ctx.egl();
    egl.destroy_surface(display, surface)
        .map_err(|e| emblema_hal::Error::Backend {
            backend: "gles",
            detail: format!("destroy_surface: {e:?}"),
        })
}
