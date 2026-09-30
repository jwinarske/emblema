//! Vulkan WSI presentation via VkSwapchainKHR.
//!
//! Surface from raw-window-handle through ash-window. FIFO by default, MAILBOX
//! when low latency is requested and available; SUBOPTIMAL and OUT_OF_DATE are
//! handled by reconfigure.

pub mod swapchain;

pub use swapchain::{PresentMode, SwapchainTarget};

/// Create a surface with no window behind it.
///
/// `VK_EXT_headless_surface` exists so that the presentation path can be
/// exercised where no display is: capability queries, format negotiation,
/// present mode selection, acquisition, presentation and recreation all behave
/// as they do against a real window, and nothing is shown.
///
/// Offered here rather than confined to a test because the value is not only in
/// testing. A conformance run on a build machine, a soak test in CI, and a
/// developer on a headless box all want the same thing, and each would
/// otherwise write this themselves.
///
/// Returns [`emblema_hal::Error::Unsupported`] where the loader does not offer
/// the extension, which is a normal thing for an older driver.
///
/// # Safety
///
/// The returned surface is the caller's to destroy, with
/// `vkDestroySurfaceKHR`, after the swapchain built on it has been destroyed.
pub fn create_headless_surface(
    ctx: &emblema_hal_vulkan::VulkanContext,
) -> emblema_hal::Result<ash::vk::SurfaceKHR> {
    let loader = ash::ext::headless_surface::Instance::new(ctx.raw_entry(), ctx.raw_instance());
    let info = ash::vk::HeadlessSurfaceCreateInfoEXT::default();
    // SAFETY: the instance outlives the surface, and the create info holds no
    // borrowed data.
    unsafe { loader.create_headless_surface(&info, None) }.map_err(|e| {
        emblema_hal::Error::Backend {
            backend: "vulkan",
            detail: format!("create_headless_surface: {e:?}"),
        }
    })
}

/// Release a surface created by [`create_headless_surface`].
///
/// # Safety
///
/// Every swapchain built on the surface must already have been destroyed.
pub unsafe fn destroy_surface(
    ctx: &emblema_hal_vulkan::VulkanContext,
    surface: ash::vk::SurfaceKHR,
) {
    let loader = ash::khr::surface::Instance::new(ctx.raw_entry(), ctx.raw_instance());
    unsafe { loader.destroy_surface(surface, None) };
}
