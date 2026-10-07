//! EGL context creation and capability detection.
//!
//! Contexts come from EGL in every configuration, including this headless one.
//! That is deliberate: the same code path serves a window surface, a GBM
//! surface for direct scanout, and the surfaceless context tests run on, so
//! there is no test-only path that could drift from the one that ships.

use emblema_hal::{Capabilities, DmaBufSupport, Error, Result, SampleCounts, SyncSupport};
use glow::HasContext;
use std::collections::HashSet;
use std::sync::Mutex;

/// EGL 1.5 rather than 1.4, because platform displays are what select the
/// surfaceless, GBM, or window platform explicitly. The 1.4 entry point guesses
/// from the native display type, which is exactly the ambiguity the DRM path
/// cannot afford.
pub type Egl = khronos_egl::DynamicInstance<khronos_egl::EGL1_5>;

/// `EGL_PLATFORM_SURFACELESS_MESA`, for a context with no drawable at all.
const PLATFORM_SURFACELESS: khronos_egl::Enum = 0x31DD;

/// `EGL_PLATFORM_GBM_KHR`, for a display obtained from a DRM render node.
///
/// The same value as `EGL_PLATFORM_GBM_MESA`; the two names were reconciled
/// and the enum never changed.
const PLATFORM_GBM: khronos_egl::Enum = 0x31D7;

/// Extensions that decide which DRM presentation path is available.
mod ext {
    /// Import a dma-buf as an EGLImage, then bind it as a texture.
    pub const DMA_BUF_IMPORT: &str = "EGL_EXT_image_dma_buf_import";
    /// Export an EGLImage back out as a dma-buf.
    pub const DMA_BUF_EXPORT: &str = "EGL_MESA_image_dma_buf_export";
    /// Query the modifiers a driver accepts, rather than assuming linear.
    pub const DMA_BUF_MODIFIERS: &str = "EGL_EXT_image_dma_buf_import_modifiers";
    /// Turn a GL fence into a sync_file fd, which is what keeps the DRM frame
    /// loop explicit rather than falling back to a CPU wait.
    pub const NATIVE_FENCE_SYNC: &str = "EGL_ANDROID_native_fence_sync";

    /// The blend equations a fixed-function unit cannot express as factors.
    pub const BLEND_ADVANCED: &str = "GL_KHR_blend_equation_advanced";

    /// The same, with overlapping draws blending in order without help.
    ///
    /// Where it is absent, an advanced equation needs `glBlendBarrierKHR`
    /// before it against anything already written to the destination -- which
    /// includes the pass's clear, not only an earlier overlapping draw. This
    /// machine's driver has the first and not this, and without the barrier it
    /// returns the source unblended for every advanced mode.
    pub const BLEND_ADVANCED_COHERENT: &str = "GL_KHR_blend_equation_advanced_coherent";

    /// Rendering into a half-float or float color attachment. Core ES 3.0 can
    /// sample one and cannot draw into it; either of these adds the second, and
    /// the half-float one is the weaker and is enough.
    pub const COLOR_BUFFER_FLOAT: &str = "GL_EXT_color_buffer_float";
    pub const COLOR_BUFFER_HALF_FLOAT: &str = "GL_EXT_color_buffer_half_float";
}

/// Which extensions a withheld capability is backed by on this backend.
///
/// Exhaustive on purpose, for the reason the Vulkan map beside it is: a new
/// `Capability` fails to compile here until someone decides what it means for
/// GLES. Unlike Vulkan, both of today's capabilities have an extension behind
/// them, so neither needs the field cleared afterwards -- though it is, because
/// doing so is free and keeps the two backends saying the same thing.
fn withheld_extensions(capability: emblema_hal::Capability) -> &'static [&'static str] {
    match capability {
        emblema_hal::Capability::AdvancedBlend => {
            &[ext::BLEND_ADVANCED, ext::BLEND_ADVANCED_COHERENT]
        }
        emblema_hal::Capability::FloatRenderTargets => {
            &[ext::COLOR_BUFFER_FLOAT, ext::COLOR_BUFFER_HALF_FLOAT]
        }
    }
}

/// How the context gets its display.
///
/// Both of these render into framebuffer objects and bind no surface; they
/// differ only in how the display handle is obtained, and that difference is
/// what decides whether a vendor driver can be used at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum DisplayTarget {
    /// No drawable. Rendering goes to framebuffer objects, which is all the
    /// offscreen executor and the golden corpus need.
    ///
    /// Needs `EGL_MESA_platform_surfaceless`, which is a Mesa extension. No
    /// vendor stack has it, so this reaches Mesa drivers and nothing else.
    #[default]
    Surfaceless,
    /// A display from a DRM render node, through GBM.
    ///
    /// The same rendering, reached a way vendor drivers support. A Vivante
    /// GC7000UL and a Qualcomm Adreno both ship `libgbm` and advertise
    /// `EGL_KHR_platform_gbm` while having no surfaceless *platform*, so this
    /// is the difference between their GLES being testable and being reported
    /// absent. Neither needs a surface: `EGL_KHR_surfaceless_context` is
    /// present on both, and it is the context extension rather than the
    /// platform one.
    ///
    /// The node is `EMBLEMA_GBM_NODE` when set, and otherwise the first
    /// `/dev/dri/renderD*` that GBM accepts.
    Gbm,
}

/// A GLES context and what it can do.
pub struct GlesContext {
    gl: glow::Context,
    /// Shared rather than owned, because a fence outlives the call that made
    /// it and needs the same instance to wait on and destroy its sync object.
    egl: std::sync::Arc<Egl>,
    display: khronos_egl::Display,
    context: khronos_egl::Context,
    /// Kept because a presentation target needs it to create a surface: a
    /// surface and the context drawing into it must share a config, and only
    /// the context knows which one it chose.
    config: khronos_egl::Config,
    capabilities: Capabilities,
    egl_extensions: HashSet<String>,
    gl_extensions: HashSet<String>,
    /// `Some` when an advanced blend needs a barrier before each draw.
    ///
    /// `None` covers both reasons there is nothing to call: the device has no
    /// advanced blending at all, or it has the coherent variant and orders the
    /// draws itself. Resolved once here rather than per draw, since a proc
    /// address does not change under a context.
    blend_barrier: Option<BlendBarrier>,
    /// Which way the display was obtained, after any fallback.
    display_target: DisplayTarget,
    /// The GBM device the display was derived from, where it came from one.
    ///
    /// A field rather than a local so it outlives the display. Fields drop
    /// after the `Drop` body, which is where the display is released, so the
    /// order is right without saying so.
    _gbm: Option<GbmDevice>,
    program: Option<crate::render::SolidProgram>,
    /// Fragment programs a caller registered, by the index they were given.
    ///
    /// Linked when registered rather than at first use, unlike the other
    /// backend: a GL program is the whole of what a draw needs, where a
    /// pipeline there also needs a render pass and a blend mode that only a
    /// draw knows.
    /// Each registered runtime program, plain and blend-qualified.
    ///
    /// Two objects rather than one because declaring
    /// `blend_support_all_equations` is not free of consequence for a draw that
    /// is not using an advanced equation: the driver here renders such a draw
    /// measurably differently, enough to move a corpus scene past its
    /// cross-backend tolerance. So the qualified program is used only where an
    /// advanced equation is in force. The second is `None` on a device without
    /// the extension, where no draw can ask for one.
    pub(crate) runtime_programs: Vec<(glow::Program, Option<glow::Program>)>,
    /// The source each was linked from, so registering it again is recognized.
    runtime_sources: Vec<String>,
    /// A one-pixel opaque white texture, bound where a draw samples nothing.
    ///
    /// Created on first use rather than eagerly, so a context that only ever
    /// fills shapes allocates nothing for a feature it does not use. White
    /// rather than transparent so that binding it in place of a real texture
    /// shows up as a blank shape rather than as nothing at all.
    placeholder: Option<glow::Texture>,
    /// Driver diagnostics, where debug output was asked for and available.
    debug: Option<std::sync::Arc<crate::debug::DebugLog>>,
}

/// How a context is created.
///
/// Mirrors the Vulkan backend's, and for the same reason: the checking costs
/// real time per call, so it is asked for rather than assumed, and tests are
/// what ask.
#[derive(Debug, Clone, Copy, Default)]
pub struct GlesConfig {
    pub target: DisplayTarget,
    /// Request `GL_KHR_debug` and capture what the driver reports.
    pub debug: bool,
    /// Capabilities to build this context without, whatever the driver offers.
    ///
    /// Empty by default, and the Vulkan backend's field of the same name says
    /// what it is for. Honored here by taking the extensions out of the set the
    /// probe reads, so the capability is false because the extension is not
    /// there -- which is the position this backend is already in on a driver
    /// that lacks it.
    pub withheld: emblema_hal::Withheld,
}

impl GlesContext {
    pub fn new(target: DisplayTarget) -> Result<Self> {
        Self::with_config(GlesConfig {
            target,
            debug: false,
            ..Default::default()
        })
    }

    /// Create a context with explicit configuration.
    ///
    /// Named `settings` rather than `config` because an EGL config is a
    /// different thing that this function also has to hold.
    pub fn with_config(settings: GlesConfig) -> Result<Self> {
        let target = settings.target;
        // SAFETY: loads libEGL by soname and keeps it alive for the lifetime of
        // the returned instance, which this struct owns.
        let egl = unsafe { Egl::load_required() }.map_err(|e| Error::Backend {
            backend: "gles",
            detail: format!("loading libEGL with EGL 1.5 entry points: {e}"),
        })?;

        let client_extensions = egl
            .query_string(None, khronos_egl::EXTENSIONS)
            .map(|s| split_extensions(&s.to_string_lossy()))
            .unwrap_or_default();

        // Held for as long as the display is, because the display is derived
        // from it: destroying the GBM device first leaves EGL holding a
        // dangling native handle.
        let mut gbm = None;
        // Surfaceless is a preference, not a demand. Asking for it on a stack
        // that has no Mesa platform used to be a hard refusal, and that is how
        // two boards with perfectly good GLES came to be recorded as having
        // none: the backend never got as far as asking them anything. Where
        // the Mesa platform is absent and GBM is there, take GBM -- the
        // resulting context is the same context, since neither binds a
        // surface. Which one was used is [`GlesContext::display_target`], so a
        // report says it rather than leaving it to be inferred.
        let target = match target {
            DisplayTarget::Surfaceless
                if !client_extensions.contains("EGL_MESA_platform_surfaceless") =>
            {
                DisplayTarget::Gbm
            }
            other => other,
        };
        let display = match target {
            DisplayTarget::Surfaceless => {
                if !client_extensions.contains("EGL_MESA_platform_surfaceless") {
                    return Err(Error::Unsupported("EGL_MESA_platform_surfaceless"));
                }
                // SAFETY: the null display argument is what the surfaceless
                // platform defines as its default, and the attribute list is
                // terminated.
                unsafe {
                    egl.get_platform_display(
                        PLATFORM_SURFACELESS,
                        khronos_egl::DEFAULT_DISPLAY,
                        &[khronos_egl::ATTRIB_NONE],
                    )
                }
                .map_err(|e| backend_err("get_platform_display", e))?
            }
            DisplayTarget::Gbm => {
                // Either spelling: the extension was promoted from MESA to KHR
                // and drivers advertise one, the other, or both.
                if !client_extensions.contains("EGL_KHR_platform_gbm")
                    && !client_extensions.contains("EGL_MESA_platform_gbm")
                {
                    return Err(Error::Unsupported("EGL_KHR_platform_gbm"));
                }
                let device = GbmDevice::open()?;
                let native = device.as_ptr();
                gbm = Some(device);
                // SAFETY: `native` is a live `gbm_device` owned by `gbm`,
                // which outlives the display, and the attribute list is
                // terminated.
                unsafe {
                    egl.get_platform_display(PLATFORM_GBM, native, &[khronos_egl::ATTRIB_NONE])
                }
                .map_err(|e| backend_err("get_platform_display (gbm)", e))?
            }
        };

        acquire_display(&egl, display)?;

        let egl_extensions = egl
            .query_string(Some(display), khronos_egl::EXTENSIONS)
            .map(|s| split_extensions(&s.to_string_lossy()))
            .unwrap_or_default();

        // A surfaceless context still needs a config; it simply never gets a
        // surface bound to it.
        if !egl_extensions.contains("EGL_KHR_surfaceless_context") {
            release_display(&egl, display);
            return Err(Error::Unsupported("EGL_KHR_surfaceless_context"));
        }

        egl.bind_api(khronos_egl::OPENGL_ES_API)
            .map_err(|e| backend_err("bind_api", e))?;

        // No surface is ever created on either target, but a config still has
        // to name a surface type it *could* serve, and the two platforms offer
        // different ones: the surfaceless platform's configs are pbuffer-
        // capable, GBM's are window-capable because its drawable is a
        // `gbm_surface`. Asking for a pbuffer on GBM matches no config at all,
        // which reads as "this driver has no ES3 RGBA8" and is nothing of the
        // kind.
        let surface_type = match target {
            DisplayTarget::Surfaceless => khronos_egl::PBUFFER_BIT,
            DisplayTarget::Gbm => khronos_egl::WINDOW_BIT,
        };
        let config_attrs = [
            khronos_egl::SURFACE_TYPE,
            surface_type,
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
        ];
        let config = match egl.choose_first_config(display, &config_attrs) {
            Ok(Some(config)) => config,
            Ok(None) => {
                release_display(&egl, display);
                return Err(Error::Unsupported("no EGL config with an ES3 RGBA8 target"));
            }
            Err(e) => {
                release_display(&egl, display);
                return Err(backend_err("choose_first_config", e));
            }
        };

        // ES 3.0 is the floor the project targets. A driver offering more
        // reports it through GL_VERSION and features are used from there.
        let context_attrs = [
            khronos_egl::CONTEXT_MAJOR_VERSION,
            3,
            khronos_egl::CONTEXT_MINOR_VERSION,
            0,
            khronos_egl::NONE,
        ];
        let context = match egl.create_context(display, config, None, &context_attrs) {
            Ok(c) => c,
            Err(e) => {
                release_display(&egl, display);
                return Err(backend_err("create_context", e));
            }
        };

        if let Err(e) = egl.make_current(display, None, None, Some(context)) {
            let _ = egl.destroy_context(display, context);
            release_display(&egl, display);
            return Err(backend_err("make_current", e));
        }

        // SAFETY: a context is current on this thread, so entry points resolved
        // here are valid for it.
        let mut gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name)
                    .map_or(std::ptr::null(), |p| p as *const std::ffi::c_void)
            })
        };

        // Taken out before the probe reads the set, so `advanced_blend` and
        // `float_render_targets` come back false for the same reason they would
        // on a driver without the extensions. The set is kept and backs
        // `has_gl_extension` and the coherent check below, so one subtraction
        // here makes all three agree.
        let mut gl_extensions = gl_extension_set(&gl);
        for capability in [
            emblema_hal::Capability::AdvancedBlend,
            emblema_hal::Capability::FloatRenderTargets,
        ] {
            if settings.withheld.contains(capability) {
                for name in withheld_extensions(capability) {
                    gl_extensions.remove(*name);
                }
            }
        }
        let gl_extensions = gl_extensions;

        let mut capabilities = detect_capabilities(&gl, &egl, &egl_extensions, &gl_extensions);
        // Before the barrier is resolved below, not after: a restricted context
        // should hold no entry point it will never call, which is what a driver
        // without the extension leaves it holding.
        settings.withheld.apply_to(&mut capabilities);

        // Only where the equations are available and the driver does not order
        // overlapping draws itself. A missing entry point where the extension
        // said there would be one leaves this `None`, which is the same
        // position the backend was in before advanced blending worked here --
        // and `advanced_blend` is turned off with it, since an equation whose
        // ordering cannot be arranged is one this backend cannot honor.
        let blend_barrier = (capabilities.advanced_blend
            && !gl_extensions.contains(ext::BLEND_ADVANCED_COHERENT))
        .then(|| {
            egl.get_proc_address("glBlendBarrierKHR")
                // SAFETY: `glBlendBarrierKHR` takes and returns nothing, and
                // the pointer came from the loader for a current context.
                .map(|proc| unsafe { std::mem::transmute::<_, BlendBarrier>(proc) })
        });
        if matches!(blend_barrier, Some(None)) {
            capabilities.advanced_blend = false;
        }
        let blend_barrier = blend_barrier.flatten();

        // Asked for and available are separate questions, and a driver without
        // debug output still renders. `debug_active` reports which happened, so
        // a caller asserting on the log can tell "clean" from "not looking".
        let debug = if settings.debug && gl_extensions.contains("GL_KHR_debug") {
            let log = std::sync::Arc::new(crate::debug::DebugLog::default());
            // SAFETY: the context was made current above and stays current on
            // this thread for as long as it lives.
            unsafe { crate::debug::install(&mut gl, log.clone()) };
            Some(log)
        } else {
            None
        };

        Ok(Self {
            gl,
            display_target: target,
            _gbm: gbm,
            egl: std::sync::Arc::new(egl),
            display,
            context,
            config,
            capabilities,
            egl_extensions,
            gl_extensions,
            blend_barrier,
            program: None,
            runtime_programs: Vec::new(),
            runtime_sources: Vec::new(),
            placeholder: None,
            debug,
        })
    }

    /// How this context's display was actually obtained.
    ///
    /// Not always what was asked for: a request for
    /// [`DisplayTarget::Surfaceless`] on a stack without the Mesa platform
    /// falls back to [`DisplayTarget::Gbm`], which is what makes a vendor
    /// driver reachable at all.
    pub fn display_target(&self) -> DisplayTarget {
        self.display_target
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// Whether the driver is reporting diagnostics into this context's log.
    ///
    /// False where debug output was not asked for, and also where it was asked
    /// for and the driver has none. The distinction matters to a caller that
    /// asserts the log is clean: an empty log means nothing at all if nothing
    /// was ever going to be written to it.
    pub fn debug_active(&self) -> bool {
        self.debug.is_some()
    }

    /// Everything the driver has reported for this context so far.
    pub fn debug_messages(&self) -> Vec<crate::debug::DebugMessage> {
        self.debug
            .as_ref()
            .map(|log| log.messages())
            .unwrap_or_default()
    }

    /// Whether the driver has reported no errors.
    /// A handle on the log that outlives this context.
    ///
    /// The same shape as the other backend's, and for the same reason: a check
    /// that reads the log through the context cannot cover what the context
    /// does on its way out. Here that is the deletion of its own program,
    /// buffers and placeholder, which happen while the context is still current
    /// and so can still raise something the callback sees.
    pub fn debug_log(&self) -> std::sync::Arc<crate::debug::DebugLog> {
        self.debug
            .clone()
            .unwrap_or_else(|| std::sync::Arc::new(crate::debug::DebugLog::default()))
    }

    pub fn debug_clean(&self) -> bool {
        self.debug.as_ref().is_none_or(|log| log.is_clean())
    }

    /// A one-pixel opaque white texture, for draws that sample nothing.
    pub(crate) fn placeholder_texture(&mut self) -> Result<glow::Texture> {
        if let Some(texture) = self.placeholder {
            return Ok(texture);
        }
        let gl = self.raw_gl();
        // SAFETY: a context is current, and the source is sized for the one
        // pixel being written.
        let texture = unsafe {
            let texture = gl
                .create_texture()
                .map_err(|e| crate::render::gl_err("create_texture", &e))?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                1,
                1,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                Some(&[255u8, 255, 255, 255]),
            );
            // Filtering must be set explicitly: a texture with the default
            // mipmap filter and no mipmaps is incomplete and samples as black,
            // which would make the placeholder do exactly what it exists to
            // avoid.
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            for axis in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, axis, glow::CLAMP_TO_EDGE as i32);
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            texture
        };
        self.placeholder = Some(texture);
        Ok(texture)
    }

    /// Link a caller's fragment program and return the name for it.
    pub fn register_program(&mut self, program: &emblema_hal::RuntimeProgram) -> Result<u32> {
        if program.glsl_es.is_empty() {
            return Err(emblema_hal::Error::Unsupported(
                "a runtime program needs GLSL ES source for this backend",
            ));
        }
        // The same source gives the same name back, so a caller with nowhere
        // to keep an index does not link a program per frame. The source is
        // kept beside the object to answer that.
        if let Some(existing) = self
            .runtime_sources
            .iter()
            .position(|held| held == &program.glsl_es)
        {
            return Ok(existing as u32);
        }
        // SAFETY: a context is current for this context's whole life.
        let linked = crate::render::build_runtime_program(&self.gl, &program.glsl_es, false)?;
        let qualified = if self.capabilities.advanced_blend {
            Some(crate::render::build_runtime_program(
                &self.gl,
                &program.glsl_es,
                true,
            )?)
        } else {
            None
        };
        let linked = (linked, qualified);
        self.runtime_programs.push(linked);
        self.runtime_sources.push(program.glsl_es.clone());
        Ok((self.runtime_programs.len() - 1) as u32)
    }

    pub(crate) fn program(&self) -> Option<&crate::render::SolidProgram> {
        self.program.as_ref()
    }

    pub(crate) fn set_program(&mut self, program: crate::render::SolidProgram) {
        self.program = Some(program);
    }

    /// The EGL entry points, display, context and config.
    ///
    /// Handed out for a presentation target to build a surface with. Grouped
    /// rather than returned one at a time because a caller needs all four
    /// together and any three of them are useless.
    pub fn egl(
        &self,
    ) -> (
        &Egl,
        khronos_egl::Display,
        khronos_egl::Context,
        khronos_egl::Config,
    ) {
        (&self.egl, self.display, self.context, self.config)
    }

    /// Bind a surface as the drawable, or unbind with `None`.
    ///
    /// A context created surfaceless can still have a surface made current
    /// later; that is what a window target does when it is created, and what it
    /// undoes when it is destroyed. Doing it here rather than in the target
    /// keeps every `eglMakeCurrent` this crate performs in one file, which is
    /// what makes "which surface is current" answerable.
    pub fn make_surface_current(&self, surface: Option<khronos_egl::Surface>) -> Result<()> {
        self.egl
            .make_current(self.display, surface, surface, Some(self.context))
            .map_err(|e| backend_err("make_current", e))
    }

    pub fn raw_gl(&self) -> &glow::Context {
        &self.gl
    }

    pub fn has_egl_extension(&self, name: &str) -> bool {
        self.egl_extensions.contains(name)
    }

    /// The blend barrier to call before an advanced blend, if one is needed.
    pub(crate) fn blend_barrier(&self) -> Option<BlendBarrier> {
        self.blend_barrier
    }

    /// Whether a GL-side extension is present.
    ///
    /// Kept queryable rather than folded into [`Capabilities`] until something
    /// acts on one. `GL_EXT_multisampled_render_to_texture` is the first that
    /// will: on a tiler it resolves inside tile memory and needs no separate
    /// multisample buffer, which changes how a multisampled pass is built
    /// rather than only how fast it runs.
    pub fn has_gl_extension(&self, name: &str) -> bool {
        self.gl_extensions.contains(name)
    }
}

impl Drop for GlesContext {
    fn drop(&mut self) {
        if let Some(program) = self.program.take() {
            use glow::HasContext;
            // SAFETY: the context is still current here, and nothing else holds
            // these objects.
            unsafe {
                self.gl.delete_program(program.program);
                if let Some(advanced) = program.advanced {
                    self.gl.delete_program(advanced);
                }
                self.gl.delete_vertex_array(program.vao);
                self.gl.delete_buffer(program.vertices);
                self.gl.delete_buffer(program.indices);
                self.gl.delete_buffer(program.paints);
            }
        }
        // SAFETY: the same context is still current.
        unsafe {
            for program in std::mem::take(&mut self.runtime_programs)
                .into_iter()
                .flat_map(|(plain, qualified)| [Some(plain), qualified])
                .flatten()
            {
                self.gl.delete_program(program);
            }
            // The placeholder is created on the first draw that samples
            // nothing, which is why it was missing here: an object built lazily
            // is easy to leave out of a teardown written when it did not exist.
            // The same omission on the other backend left a descriptor set
            // layout outliving its device, which is undefined behavior rather
            // than a tidiness question -- here it is neither, since destroying
            // the EGL context below releases everything it owns. It is deleted
            // anyway, because "the context releases what it made" is a rule
            // worth being able to state without an exception in it.
            if let Some(texture) = self.placeholder.take() {
                self.gl.delete_texture(texture);
            }
        }
        // Unbind before destroying, or the driver keeps the context alive and
        // the display never actually releases its resources.
        let _ = self.egl.make_current(self.display, None, None, None);
        let _ = self.egl.destroy_context(self.display, self.context);

        // Terminating is refcounted across contexts, because an EGL display is
        // process-global: asking for the same platform twice returns the same
        // handle, and terminating it invalidates every context on it, not just
        // this one. Doing it unconditionally tore down contexts belonging to
        // other live users, which showed up as a second context reporting that
        // the display had no extensions at all.
        release_display(&self.egl, self.display);
    }
}

/// A `gbm_device`, and the library it came from.
///
/// `libgbm` is opened at runtime rather than linked, for the reason the
/// manifest gives for `libEGL`: linking it would need the library and
/// `pkg-config` on the build host and break cross-building to a board, which
/// is the only place this variant is useful.
///
/// Two symbols are all a display needs. Surfaces and buffer objects belong to
/// a presentation path; this one renders into framebuffer objects.
struct GbmDevice {
    /// Dropped last, after the device it provided the destructor for.
    _library: libloading::Library,
    device: *mut std::ffi::c_void,
    destroy: unsafe extern "C" fn(*mut std::ffi::c_void),
    /// The node stays open for the device's lifetime; GBM does not dup it.
    _node: std::fs::File,
}

impl GbmDevice {
    fn open() -> Result<Self> {
        // SAFETY: both names are a plain shared library, and the symbols below
        // are looked up before any is called.
        let library = unsafe { libloading::Library::new("libgbm.so.1") }
            .or_else(|_| unsafe { libloading::Library::new("libgbm.so") })
            .map_err(|e| Error::Backend {
                backend: "gles",
                detail: format!("dlopen libgbm: {e}"),
            })?;
        // SAFETY: these are `gbm_create_device` and `gbm_device_destroy` with
        // the signatures libgbm has had since it existed.
        let (create, destroy) = unsafe {
            let create: libloading::Symbol<
                unsafe extern "C" fn(std::ffi::c_int) -> *mut std::ffi::c_void,
            > = library
                .get(b"gbm_create_device\0")
                .map_err(|e| Error::Backend {
                    backend: "gles",
                    detail: format!("gbm_create_device: {e}"),
                })?;
            let destroy: libloading::Symbol<unsafe extern "C" fn(*mut std::ffi::c_void)> = library
                .get(b"gbm_device_destroy\0")
                .map_err(|e| Error::Backend {
                    backend: "gles",
                    detail: format!("gbm_device_destroy: {e}"),
                })?;
            (*create, *destroy)
        };

        let mut refused = Vec::new();
        for path in gbm_nodes() {
            let node = match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
            {
                Ok(node) => node,
                Err(e) => {
                    refused.push(format!("{path}: {e}"));
                    continue;
                }
            };
            use std::os::fd::AsRawFd as _;
            // SAFETY: the descriptor is open and stays open in `node` below.
            let device = unsafe { create(node.as_raw_fd()) };
            if device.is_null() {
                refused.push(format!("{path}: gbm_create_device returned null"));
                continue;
            }
            return Ok(Self {
                _library: library,
                device,
                destroy,
                _node: node,
            });
        }
        Err(Error::Backend {
            backend: "gles",
            detail: format!("no DRM node GBM would take ({})", refused.join("; ")),
        })
    }

    fn as_ptr(&self) -> *mut std::ffi::c_void {
        self.device
    }
}

impl Drop for GbmDevice {
    fn drop(&mut self) {
        // SAFETY: the device came from `gbm_create_device` and is destroyed
        // once, after the display built on it has been released.
        unsafe { (self.destroy)(self.device) };
    }
}

/// Render nodes to offer GBM, in the order they are tried.
///
/// A render node rather than a card: GBM wants the rendering device, and on a
/// split render/display part the card node is the display controller, which
/// cannot allocate for the GPU. `EMBLEMA_GBM_NODE` names one outright, for a
/// board with more than one GPU or to force the choice.
fn gbm_nodes() -> Vec<String> {
    if let Ok(named) = std::env::var("EMBLEMA_GBM_NODE") {
        return vec![named];
    }
    let mut nodes: Vec<String> = std::fs::read_dir("/dev/dri")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("renderD"))
        .map(|name| format!("/dev/dri/{name}"))
        .collect();
    nodes.sort();
    nodes
}

/// How many live contexts each process-global display has.
///
/// A count rather than a flag: the last user out is the one that may terminate,
/// and a flag could not tell the difference between one user and several.
static DISPLAY_USERS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// Record a new user of a display, initializing it on first use.
fn acquire_display(egl: &Egl, display: khronos_egl::Display) -> Result<()> {
    let key = display.as_ptr() as usize;
    let mut users = DISPLAY_USERS.lock().unwrap_or_else(|e| e.into_inner());
    match users.iter_mut().find(|(handle, _)| *handle == key) {
        Some((_, count)) => *count += 1,
        None => {
            // Initializing more than once is permitted and is what makes the
            // count meaningful, but doing it under the lock keeps a second
            // thread from terminating between the two.
            egl.initialize(display)
                .map_err(|e| backend_err("initialize", e))?;
            users.push((key, 1));
        }
    }
    Ok(())
}

/// Drop a user, terminating the display once the last one goes.
fn release_display(egl: &Egl, display: khronos_egl::Display) {
    let key = display.as_ptr() as usize;
    let mut users = DISPLAY_USERS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(index) = users.iter().position(|(handle, _)| *handle == key) {
        users[index].1 -= 1;
        if users[index].1 == 0 {
            users.swap_remove(index);
            let _ = egl.terminate(display);
        }
    }
}

impl std::fmt::Debug for GlesContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlesContext")
            .field("device", &self.capabilities.device_name)
            .field("driver", &self.capabilities.driver_name)
            .finish_non_exhaustive()
    }
}

fn backend_err(what: &str, e: khronos_egl::Error) -> Error {
    Error::Backend {
        backend: "gles",
        detail: format!("{what}: {e}"),
    }
}

fn split_extensions(list: &str) -> HashSet<String> {
    list.split_whitespace().map(|s| s.to_owned()).collect()
}

fn gl_extension_set(gl: &glow::Context) -> HashSet<String> {
    // SAFETY: a context is current, and the count is queried before indexing.
    unsafe {
        let count = gl.get_parameter_i32(glow::NUM_EXTENSIONS).max(0) as u32;
        (0..count)
            .map(|i| gl.get_parameter_indexed_string(glow::EXTENSIONS, i))
            .collect()
    }
}

/// `glBlendBarrierKHR`, which glow does not wrap.
///
/// Needed only where `GL_KHR_blend_equation_advanced` is present without its
/// coherent variant. It orders one draw's blend against the pixels an earlier
/// draw in the same pass wrote; without it, an advanced equation may read a
/// destination that has not settled.
pub(crate) type BlendBarrier = unsafe extern "system" fn();

/// `glGetInternalformativ`, which glow does not wrap.
///
/// Core since ES 3.0, so the entry point is present on every context this
/// backend will run on; the fallback below exists because a null proc address
/// is easier to handle than to prove impossible.
type GetInternalformativ = unsafe extern "system" fn(u32, u32, u32, i32, *mut i32);

/// The multisample counts a renderbuffer of `format` can actually be given.
///
/// Returns a mask whose set bits are the counts themselves, so 4 and 8 give
/// `0b1100`. One is not included: a single-sampled attachment is allocated
/// through `RenderbufferStorage` and is not a sample count this query answers.
fn format_sample_mask(query: GetInternalformativ, format: u32) -> u32 {
    let mut count = 0i32;
    // SAFETY: the entry point was resolved from a current context, and each
    // call is given a buffer of exactly the length it was told to write.
    unsafe {
        query(
            glow::RENDERBUFFER,
            format,
            glow::NUM_SAMPLE_COUNTS,
            1,
            &mut count,
        );
    }
    if count <= 0 {
        return 0;
    }
    let mut counts = vec![0i32; count as usize];
    unsafe {
        query(
            glow::RENDERBUFFER,
            format,
            glow::SAMPLES,
            count,
            counts.as_mut_ptr(),
        );
    }
    counts
        .into_iter()
        .filter(|c| (2..=16).contains(c))
        .map(|c| c as u32)
        .filter(|c| c.is_power_of_two())
        .fold(0, |mask, c| mask | c)
}

/// The sample counts a multisampled pass can use, asked rather than assumed.
///
/// This used to read `MAX_SAMPLES` and take every power of two up to it, on the
/// stated grounds that the specification defines the limit that way and that
/// every driver behaves accordingly. Both halves are wrong, and both of the
/// drivers available here falsify them: Mesa's llvmpipe supports 4 and 8 for
/// `RGBA8` and not 2, and radeonsi supports 2, 4 and 8 but not 1.
///
/// What makes it worth querying rather than tolerating is that asking for an
/// unsupported count is not an error. `RenderbufferStorageMultisample` rounds
/// the request up to the next count the format does support, so a caller who
/// budgets for 2x gets 4x -- twice the memory and twice the resolve bandwidth
/// -- while `Capabilities` reports that it got what it asked for. On the
/// embedded targets this renderer exists to run on, that is not a rounding
/// detail.
///
/// The counts reported are those a color attachment and the stencil beside it
/// both support, because a multisampled pass allocates both and a framebuffer
/// whose attachments disagree about sample count is incomplete rather than
/// slow. Formats outside the eight-bit pair are not folded in: intersecting
/// every renderable format would let a rarely-used one narrow the answer for
/// the common case, so those are caught at allocation instead, where the
/// realized count is checked against the requested one.
fn multisample_mask(egl: &Egl, max_samples: u32) -> u32 {
    let Some(proc) = egl.get_proc_address("glGetInternalformativ") else {
        // No query, so the old assumption is all that is left. It is stated
        // here as an assumption rather than presented as a limit.
        let mut mask = 1u32;
        let mut n = 2u32;
        while n <= max_samples && n <= 16 {
            mask |= n;
            n <<= 1;
        }
        return mask;
    };
    // SAFETY: `glGetInternalformativ` has this signature in ES 3.0 and every
    // later version, and the pointer came from the loader for a current
    // context.
    let query: GetInternalformativ = unsafe { std::mem::transmute(proc) };
    let color =
        format_sample_mask(query, glow::RGBA8) & format_sample_mask(query, glow::SRGB8_ALPHA8);
    // Single-sampled is always available and is not a multisample count.
    1 | (color & format_sample_mask(query, glow::STENCIL_INDEX8))
}

fn detect_capabilities(
    gl: &glow::Context,
    egl: &Egl,
    egl_extensions: &HashSet<String>,
    gl_extensions: &HashSet<String>,
) -> Capabilities {
    // SAFETY: a context is current on this thread.
    let (max_texture_size, max_samples, renderer, version) = unsafe {
        (
            gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE).max(0) as u32,
            gl.get_parameter_i32(glow::MAX_SAMPLES).max(1) as u32,
            gl.get_parameter_string(glow::RENDERER),
            gl.get_parameter_string(glow::VERSION),
        )
    };

    let mask = multisample_mask(egl, max_samples);

    // Import and export are separate extensions here, unlike Vulkan where one
    // handle type covers both. A driver offering only import still supports the
    // GBM path, where GBM allocates and GLES renders into what it made.
    let dma_buf = DmaBufSupport {
        import: egl_extensions.contains(ext::DMA_BUF_IMPORT),
        export: egl_extensions.contains(ext::DMA_BUF_EXPORT),
        modifiers: egl_extensions.contains(ext::DMA_BUF_MODIFIERS),
    };

    // Follows the extension again, which it did not for a while and should
    // not have: there was no fence to export, `Hal::Fence` being
    // `std::convert::Infallible`, so reporting the extension promised a caller
    // an export it could obtain nothing to perform. Reporting false was the
    // honest reading of a backend that could not produce a fence at all.
    //
    // Now it can. `export_sync_file` follows `EGL_ANDROID_native_fence_sync`
    // because that is the sync type that dups out to a `sync_file`; a plain
    // `EGL_KHR_fence_sync` object serves both waits and cannot leave the
    // process, which is no use to an atomic commit.
    let fence = egl_extensions.contains(ext::NATIVE_FENCE_SYNC);
    Capabilities {
        // The extension requires the fragment stage to declare
        // `blend_support_all_equations`, which naga cannot emit from WGSL --
        // recorded here for a while as needing a hand-written GLSL stage, and
        // it does not: the declaration and the `#extension` that legalizes it
        // are two lines after the version line, inserted by
        // `render::with_advanced_blend` the way the shader crate's build
        // script already inserts a `std140` qualifier.
        //
        // Reporting the extension is therefore honest here, where reporting it
        // before would have produced plain source-over silently.
        advanced_blend: gl_extensions.contains(ext::BLEND_ADVANCED),
        max_texture_size,
        sample_counts: SampleCounts::from_mask(mask),
        dma_buf,
        sync: SyncSupport {
            export_sync_file: fence,
            // Not the same answer, and deliberately. Export is implemented:
            // `GlesFence` creates a native fence and dups it out. Import is
            // not -- it needs a sync built *from* a descriptor, through the
            // `EGL_SYNC_NATIVE_FENCE_FD_ANDROID` attribute, and there is no
            // constructor here that takes one. Reporting it from the extension
            // would promise a wait nothing can be handed to, which is the
            // exact mistake this pair carried before either half worked.
            import_sync_file: false,
        },
        // Core ES 3.0 can sample a half-float texture and cannot render into
        // one; either extension adds the second. Named separately because the
        // half-float one is the weaker of the two and is enough for this.
        float_render_targets: gl_extensions.contains(ext::COLOR_BUFFER_FLOAT)
            || gl_extensions.contains(ext::COLOR_BUFFER_HALF_FLOAT),
        // Empty, and the only field here that is empty without a reason
        // beside it. It is not a statement that this backend cannot export a
        // scanout buffer -- `dma_buf` above says it can, wherever the three
        // EGL extensions are present, and on a Raspberry Pi's V3D all three
        // are. It is a list nobody has filled in.
        //
        // What that costs is a misleading failure. `DrmScanoutTarget::new`
        // checks `can_allocate_scanout()` first, which passes here, and then
        // negotiates against this list -- so the error a caller sees is "no
        // shared format and modifier, render side: <nothing>" rather than
        // anything about GLES.
        //
        // Filling it in honestly is not the fix, though, and that was measured
        // rather than argued. Exporting a plain renderable texture on a Pi 5
        // gives `AB24` with `BROADCOM_UIF`, while the board's two display
        // controllers take `LINEAR` only (rp1-dsi) and `VC4_T_TILED` or
        // `LINEAR` (vc4). A truthful list of what this backend exports would
        // negotiate against those and still find nothing, because GL has no
        // way to *ask* for a layout when it allocates a texture. That is what
        // GBM is for, and `docs/architecture.md` has the numbers.
        render_formats: Vec::new(),
        // Recognized from the renderer string, because GLES offers nothing
        // better: there is no device-type query, and the string is what every
        // tool that needs this answer reads. Matching is on the rasterizer
        // names rather than on a word like "software", which appears in plenty
        // of hardware driver strings.
        software: ["llvmpipe", "softpipe", "swiftshader", "swrast"]
            .iter()
            .any(|name| renderer.to_ascii_lowercase().contains(name)),
        device_name: renderer,
        driver_name: version,
    }
}

impl GlesContext {
    /// Place a fence in the command stream and flush, so it can signal.
    ///
    /// Exportable where the driver offers `EGL_ANDROID_native_fence_sync`, and
    /// a plain sync object otherwise -- which still serves both waits and
    /// refuses the export, rather than refusing the whole submission. A
    /// display path checks `SyncSupport::export_sync_file` before asking.
    pub fn fence_after_submission(&mut self) -> Result<crate::fence::GlesFence> {
        let exportable = self.capabilities.sync.export_sync_file;
        let fence = crate::fence::GlesFence::create(self.egl.clone(), self.display, exportable)?;
        // After the sync, never before: the flush is what hands the driver the
        // stream the fence is sitting in.
        unsafe { self.gl.flush() };
        Ok(fence)
    }
}
