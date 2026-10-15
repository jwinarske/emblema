// Style as in the probes that use this: an example is a program, and it
// reports by printing.
//
// `dead_code` because each example compiles its own copy of this module, so a
// constructor only one of them calls is unused in the other.
#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

//! The EGL and GL bring-up the bare probes share.
//!
//! Both probes here are reproductions that the tree's own rule asks for before
//! a driver is named: EGL and GL and nothing else. Reaching a context that way
//! is about eighty lines of platform selection and `libgbm` loading, and the
//! two probes must reach the *same* one -- a difference in how the display was
//! obtained would be a difference between their readings that neither probe
//! reports. So it is written once here rather than copied.
//!
//! The context attributes match `context.rs` deliberately: ES 3.0 with an
//! RGBA8 config. A probe compiled against a different context than the renderer
//! would be asking about a different driver state.

use glow::HasContext;

/// `EGL_PLATFORM_SURFACELESS_MESA`. The target and its first control are both
/// Mesa drivers and reach the display this way.
const PLATFORM_SURFACELESS: khronos_egl::Enum = 0x31DD;

/// `EGL_PLATFORM_GBM_KHR`, which the two vendor stacks need: a GC7000UL and an
/// Adreno ship `libgbm` and advertise the GBM platform while having no
/// surfaceless *platform* at all. Four drivers is the whole point of these
/// probes, so they reach a display both ways.
const PLATFORM_GBM: khronos_egl::Enum = 0x31D7;

/// `libgbm`, opened at run time rather than linked, so this cross-builds to a
/// board without the library on the build host. Two symbols are all a display
/// needs; surfaces belong to a presentation path and these probes render into
/// framebuffer objects.
struct Gbm {
    _library: libloading::Library,
    device: *mut std::ffi::c_void,
    destroy: unsafe extern "C" fn(*mut std::ffi::c_void),
    _node: std::fs::File,
}

impl Gbm {
    /// The first render node that yields a device, or `None`.
    fn open() -> Option<Self> {
        let nodes: Vec<String> = match std::env::var("EMBLEMA_GBM_NODE") {
            Ok(named) => vec![named],
            Err(_) => {
                let mut found: Vec<String> = std::fs::read_dir("/dev/dri")
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| n.starts_with("renderD"))
                    .map(|n| format!("/dev/dri/{n}"))
                    .collect();
                found.sort();
                found
            }
        };
        // SAFETY: the symbols below are `libgbm`'s documented C entry points,
        // and the library outlives the device it creates.
        unsafe {
            let library = libloading::Library::new("libgbm.so.1").ok()?;
            let create: libloading::Symbol<
                unsafe extern "C" fn(std::os::fd::RawFd) -> *mut std::ffi::c_void,
            > = library.get(b"gbm_create_device\0").ok()?;
            let destroy: libloading::Symbol<unsafe extern "C" fn(*mut std::ffi::c_void)> =
                library.get(b"gbm_device_destroy\0").ok()?;
            let destroy = *destroy;
            for node in nodes {
                let Ok(file) = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&node)
                else {
                    continue;
                };
                let device = create(std::os::fd::AsRawFd::as_raw_fd(&file));
                if !device.is_null() {
                    println!("gbm device: {node}");
                    return Some(Self {
                        _library: library,
                        device,
                        destroy,
                        _node: file,
                    });
                }
            }
            None
        }
    }
}

impl Drop for Gbm {
    fn drop(&mut self) {
        // SAFETY: the device came from `gbm_create_device` and is destroyed
        // once.
        unsafe { (self.destroy)(self.device) };
    }
}

/// A current GLES context with advanced blending, and what a probe needs to use
/// it.
pub struct Bare {
    egl: khronos_egl::DynamicInstance<khronos_egl::EGL1_5>,
    display: khronos_egl::Display,
    context: khronos_egl::Context,
    pub gl: glow::Context,
    /// `glBlendBarrierKHR`, or `None` where the coherent variant makes it
    /// unnecessary. Overlapping draws under an advanced equation need it
    /// between them, and so does a draw over a cleared target: the clear is a
    /// write.
    pub blend_barrier: Option<extern "C" fn()>,
    /// Held because the display was taken from it and must not outlive it.
    _gbm: Option<Gbm>,
}

impl Bare {
    /// A context, or `None` with the reason printed.
    ///
    /// Printing rather than returning an error, because every caller is a
    /// program whose answer to any of these is to say so and stop: a driver
    /// without advanced blending has nothing either probe can measure.
    pub fn open() -> Option<Self> {
        Self::open_with(true)
    }

    /// The same context without the four lines naming the platform and the
    /// driver.
    ///
    /// For a probe that opens one context per measurement -- which is what
    /// measuring "what one frame leaves for the next" needs -- where the
    /// header would be printed once per run and say the same thing every time.
    /// A refusal still prints: a caller that got nothing has to know why.
    pub fn open_silent() -> Option<Self> {
        Self::open_with(false)
    }

    fn open_with(announce: bool) -> Option<Self> {
        // SAFETY: the EGL library, display and context outlive every call
        // below, and the context is current on this thread for all of the GL.
        unsafe {
            let egl = khronos_egl::DynamicInstance::<khronos_egl::EGL1_5>::load_required()
                .expect("loading libEGL");
            let client_extensions = egl
                .query_string(None, khronos_egl::EXTENSIONS)
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            // Surfaceless where it exists, GBM otherwise. Held in scope either
            // way: the device has to outlive the display taken from it.
            let mut gbm = None;
            let display = if client_extensions.contains("EGL_MESA_platform_surfaceless") {
                if announce {
                    println!("platform: surfaceless");
                }
                egl.get_platform_display(
                    PLATFORM_SURFACELESS,
                    khronos_egl::DEFAULT_DISPLAY,
                    &[khronos_egl::ATTRIB_NONE],
                )
                .expect("get_platform_display")
            } else if client_extensions.contains("EGL_KHR_platform_gbm")
                || client_extensions.contains("EGL_MESA_platform_gbm")
            {
                if announce {
                    println!("platform: gbm");
                }
                let device = Gbm::open().expect("a GBM device from a render node");
                let native = device.device;
                gbm = Some(device);
                egl.get_platform_display(PLATFORM_GBM, native, &[khronos_egl::ATTRIB_NONE])
                    .expect("get_platform_display (gbm)")
            } else {
                println!("skipping: neither the surfaceless nor the GBM platform");
                return None;
            };
            egl.initialize(display).expect("initialize");
            egl.bind_api(khronos_egl::OPENGL_ES_API).expect("bind_api");
            let config = egl
                .choose_first_config(
                    display,
                    &[
                        khronos_egl::SURFACE_TYPE,
                        // The two platforms offer different surface types, and
                        // asking GBM for a pbuffer matches no config at all --
                        // which reads as "this driver has no ES3 RGBA8" and is
                        // nothing of the kind.
                        if gbm.is_some() {
                            khronos_egl::WINDOW_BIT
                        } else {
                            khronos_egl::PBUFFER_BIT
                        },
                        khronos_egl::RENDERABLE_TYPE,
                        khronos_egl::OPENGL_ES3_BIT,
                        khronos_egl::RED_SIZE,
                        8,
                        khronos_egl::GREEN_SIZE,
                        8,
                        khronos_egl::BLUE_SIZE,
                        8,
                        khronos_egl::ALPHA_SIZE,
                        8,
                        khronos_egl::NONE,
                    ],
                )
                .expect("choose_first_config")
                .expect("an ES3 RGBA8 config");
            let context = egl
                .create_context(
                    display,
                    config,
                    None,
                    &[
                        khronos_egl::CONTEXT_MAJOR_VERSION,
                        3,
                        khronos_egl::CONTEXT_MINOR_VERSION,
                        0,
                        khronos_egl::NONE,
                    ],
                )
                .expect("create_context");
            egl.make_current(display, None, None, Some(context))
                .expect("make_current");

            let gl = glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name)
                    .map_or(std::ptr::null(), |p| p as *const std::ffi::c_void)
            });

            if announce {
                println!("renderer: {}", gl.get_parameter_string(glow::RENDERER));
                println!("version:  {}", gl.get_parameter_string(glow::VERSION));
            }
            let extensions = gl.get_parameter_string(glow::EXTENSIONS);
            let advanced = extensions.contains("GL_KHR_blend_equation_advanced");
            let coherent = extensions.contains("GL_KHR_blend_equation_advanced_coherent");
            if !advanced {
                println!("skipping: no GL_KHR_blend_equation_advanced");
                return None;
            }
            if announce {
                println!("advanced blending: yes, coherent: {coherent}");
            }

            // Without the coherent variant, overlapping draws under an
            // advanced equation need this between them.
            let blend_barrier: Option<extern "C" fn()> = if coherent {
                None
            } else {
                egl.get_proc_address("glBlendBarrierKHR")
                    .map(|p| std::mem::transmute::<_, extern "C" fn()>(p))
            };
            if !coherent && blend_barrier.is_none() {
                println!("skipping: non-coherent advanced blending and no glBlendBarrierKHR");
                return None;
            }

            Some(Self {
                egl,
                display,
                context,
                gl,
                blend_barrier,
                _gbm: gbm,
            })
        }
    }
}

impl Drop for Bare {
    fn drop(&mut self) {
        self.egl
            .destroy_context(self.display, self.context)
            .expect("destroy_context");
        self.egl.terminate(self.display).expect("terminate");
    }
}
