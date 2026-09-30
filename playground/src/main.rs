//! Look at a scene in a window, live.
//!
//! # What this is for
//!
//! The suite says whether a scene matches another backend and a software
//! reference. It does not say whether the scene looks right, and this renderer
//! has made that mistake twice in a way nothing caught: a gradient that agreed
//! across backends and went flat for the last part of its length, and a frosted
//! panel that rendered identically with the filter on and off. Both were a
//! second's work to see and invisible to a comparison between two
//! implementations making the same mistake.
//!
//! # Two kinds of scene
//!
//! The **corpus** is what the suite compares: fixed size, fixed parameters.
//! Those are rendered at their own size into a texture and drawn into the
//! window centered and scaled, because their coordinates are absolute and their
//! recordings are in clip space — submitting one straight into a window-sized
//! image would stretch it to whatever shape the window is.
//!
//! **Live** scenes are drawn at the window's own size and carry a knob. They
//! are for the questions a still image cannot answer: whether a parameter
//! sweeps smoothly, and whether something that moves every frame moves well.
//! See `live.rs`.

mod live;

use emblema::present::PresentTarget;
use emblema::vulkan::{DevicePreference, VulkanContext, VulkanHal, VulkanTexture};
use emblema::wsi::{PresentMode, SwapchainTarget};
use emblema::{Canvas, Color, Paint, Rect, TileMode};
use emblema::{Extent2D, PixelFormat, TextureDescriptor};
use emblema_testkit::{corpus, record_scene, Scene};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// Square, because most corpus scenes are, so the first view is undistorted.
const INITIAL: u32 = 768;

fn main() {
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("run");
}

struct App {
    scenes: Vec<Scene>,
    live: Vec<live::Live>,
    /// Index across the corpus and then the live scenes, so one key steps
    /// through everything.
    current: usize,
    /// One per live scene, kept while stepping away so a swept value is not
    /// lost by looking at something else.
    knobs: Vec<f32>,
    animating: bool,
    time: f32,
    stage: Option<Stage>,
}

/// Everything that needs a window, built when one arrives.
///
/// winit hands the window over in `resumed` rather than at construction, and on
/// a platform that suspends it can go away and come back.
struct Stage {
    window: Window,
    ctx: VulkanContext,
    target: SwapchainTarget,
    surface: ash::vk::SurfaceKHR,
    /// A corpus scene rendered at its own size, kept until the view changes.
    scene_texture: Option<(VulkanTexture, Extent2D)>,
    /// The sprite sheet the atlas scene reads, uploaded once and kept for the
    /// window's life.
    ///
    /// Built whether or not the scene that reads it is on screen, because it
    /// is sixteen texels and the alternative is a lazily-created resource
    /// whose absence is a rendering bug rather than an allocation saved.
    sheet: Option<VulkanTexture>,
}

enum View {
    Corpus(usize),
    Live(usize),
}

impl App {
    fn new() -> Self {
        // The corpus first, then the catalog: the corpus is the small tuned
        // set a failure names a capability in, and the catalog is the broad
        // one mirroring Impeller's own playground. Both are data, so both
        // reach the window without a line of code per scene.
        let mut scenes = corpus();
        scenes.extend(emblema_testkit::catalog());
        let live = live::scenes();
        assert!(!scenes.is_empty(), "the corpus is empty");
        let knobs = live.iter().map(|s| s.start).collect();
        Self {
            scenes,
            live,
            current: 0,
            knobs,
            animating: true,
            time: 0.0,
            stage: None,
        }
    }

    fn total(&self) -> usize {
        self.scenes.len() + self.live.len()
    }

    fn view(&self) -> View {
        if self.current < self.scenes.len() {
            View::Corpus(self.current)
        } else {
            View::Live(self.current - self.scenes.len())
        }
    }

    fn step(&mut self, forward: bool) {
        let n = self.total();
        self.current = if forward {
            (self.current + 1) % n
        } else {
            (self.current + n - 1) % n
        };
        if let Some(stage) = &mut self.stage {
            stage.drop_scene_texture();
        }
        self.announce();
        self.request_redraw();
    }

    /// Move the current live scene's knob, as a fraction of its range.
    fn turn(&mut self, fraction: f32) {
        let View::Live(index) = self.view() else {
            return;
        };
        let (low, high) = self.live[index].range;
        let step = (high - low) * fraction;
        self.knobs[index] = (self.knobs[index] + step).clamp(low, high);
        self.announce();
        self.request_redraw();
    }

    fn announce(&self) {
        match self.view() {
            View::Corpus(i) => eprintln!(
                "[{}/{}] corpus: {}",
                self.current + 1,
                self.total(),
                self.scenes[i].name
            ),
            View::Live(i) => eprintln!(
                "[{}/{}] live: {} — {} {:.2}{}",
                self.current + 1,
                self.total(),
                self.live[i].name,
                self.live[i].knob,
                self.knobs[i],
                if self.animating { ", animating" } else { "" }
            ),
        }
    }

    fn request_redraw(&self) {
        if let Some(stage) = &self.stage {
            stage.window.request_redraw();
        }
    }

    /// Whether the current view changes on its own, and so wants a new frame
    /// without an event to prompt one.
    fn is_moving(&self) -> bool {
        self.animating && matches!(self.view(), View::Live(_))
    }

    fn redraw(&mut self) {
        // Advanced here rather than from a clock, because a playground wants
        // motion that is steady to look at rather than true to wall time.
        if self.is_moving() {
            self.time += 1.0 / 60.0;
        }
        let view = self.view();
        let (time, knob) = match view {
            View::Live(i) => (self.time, self.knobs[i]),
            View::Corpus(_) => (0.0, 0.0),
        };
        let scene = match view {
            View::Corpus(i) => Some(self.scenes[i].clone()),
            View::Live(_) => None,
        };
        let draw = match view {
            View::Live(i) => Some(self.live[i].draw),
            View::Corpus(_) => None,
        };
        let Some(stage) = &mut self.stage else {
            return;
        };
        match (scene, draw) {
            (Some(scene), _) => stage.show_corpus(&scene),
            (None, Some(draw)) => {
                let extent = stage.target.extent();
                let mut canvas = Canvas::new(extent);
                draw(&mut canvas, extent, knob, time);
                let recording = canvas.finish();
                // Taken out and put back, because submitting borrows the stage
                // mutably and the sheet lives on it. A scene that never names
                // a texture slot is unaffected by one being supplied.
                let sheet = stage.sheet.take();
                match &sheet {
                    Some(sheet) => stage.submit(&recording, &[sheet]),
                    None => stage.submit(&recording, &[]),
                }
                stage.sheet = sheet;
            }
            (None, None) => {}
        }
    }
}

impl Stage {
    fn drop_scene_texture(&mut self) {
        if let Some((texture, _)) = self.scene_texture.take() {
            self.ctx.destroy_texture(texture);
        }
    }

    /// Render a corpus scene into a texture of the scene's own size.
    ///
    /// Kept until the view changes, so a resize redraws from this rather than
    /// re-recording — the scene is authored at one size and the window is
    /// showing it larger.
    fn ensure_scene_texture(&mut self, scene: &Scene) {
        if self.scene_texture.is_some() {
            return;
        }
        let extent = Extent2D::new(scene.size.width, scene.size.height);
        let recording = match record_scene(scene) {
            Ok(recording) => recording,
            Err(e) => return eprintln!("cannot record {}: {e}", scene.name),
        };
        let mut texture = match self.ctx.create_texture(&TextureDescriptor::offscreen(
            extent,
            PixelFormat::Rgba8Unorm,
        )) {
            Ok(texture) => texture,
            Err(e) => return eprintln!("cannot allocate a target for {}: {e}", scene.name),
        };
        match emblema::execute::<VulkanHal>(&mut self.ctx, &mut texture, &recording, &[]) {
            Ok(()) => self.scene_texture = Some((texture, extent)),
            Err(e) => {
                eprintln!("cannot render {}: {e}", scene.name);
                self.ctx.destroy_texture(texture);
            }
        }
    }

    /// Compose a corpus scene's texture into the window, centered and scaled.
    fn corpus_frame(&mut self, scene: &Scene) -> Option<emblema::Recording> {
        self.ensure_scene_texture(scene);
        let (_, scene_extent) = self.scene_texture.as_ref()?;
        let window = self.target.extent();
        let scale = (window.width as f32 / scene_extent.width as f32)
            .min(window.height as f32 / scene_extent.height as f32)
            .max(0.01);
        let (w, h) = (
            scene_extent.width as f32 * scale,
            scene_extent.height as f32 * scale,
        );
        let (x, y) = (
            (window.width as f32 - w) / 2.0,
            (window.height as f32 - h) / 2.0,
        );

        let mut canvas = Canvas::new(window);
        // Neither black nor white, so a scene of either is visible against it
        // and the letterboxing is obviously not part of the scene.
        canvas.clear(Color::srgb(0.12, 0.12, 0.14, 1.0));
        let rect = Rect::new(x, y, x + w, y + h);
        if let Err(e) = canvas.draw_rect(
            rect,
            &Paint::image(0, rect)
                .with_tile_mode(TileMode::Decal)
                .with_anti_alias(false),
        ) {
            eprintln!("cannot compose the frame: {e}");
            return None;
        }
        Some(canvas.finish())
    }

    /// Compose and present a corpus scene.
    ///
    /// One method rather than two, because the texture it samples belongs to
    /// this struct and the submission needs the struct mutably: splitting them
    /// leaves a borrow of the texture spanning a call that wants everything.
    fn show_corpus(&mut self, scene: &Scene) {
        let Some(recording) = self.corpus_frame(scene) else {
            return;
        };
        let Some((texture, _)) = self.scene_texture.take() else {
            return;
        };
        let images: Vec<&VulkanTexture> = vec![&texture];
        // Submitted while the texture is out of the struct, then put back --
        // the alternative is holding a borrow of it across a method that needs
        // `self` mutably, which is the same lifetime problem stated less
        // plainly.
        if let Err(e) = self.target.acquire(&mut self.ctx) {
            eprintln!("acquire: {e}");
        } else if let Err(e) = self
            .target
            .submit_recording(&mut self.ctx, &recording, &images)
        {
            eprintln!("submit: {e}");
        } else if let Err(e) = self.target.present(&mut self.ctx) {
            eprintln!("present: {e}");
        }
        let extent = Extent2D::new(scene.size.width, scene.size.height);
        self.scene_texture = Some((texture, extent));
    }

    fn submit(&mut self, recording: &emblema::Recording, images: &[&VulkanTexture]) {
        if let Err(e) = self.target.acquire(&mut self.ctx) {
            return eprintln!("acquire: {e}");
        }
        if let Err(e) = self
            .target
            .submit_recording(&mut self.ctx, recording, images)
        {
            return eprintln!("submit: {e}");
        }
        if let Err(e) = self.target.present(&mut self.ctx) {
            eprintln!("present: {e}");
        }
    }
}

/// Upload the sprite sheet the atlas scene reads.
///
/// A failure here is reported and then ignored: every other scene draws
/// without it, and losing the whole playground because sixteen texels would
/// not upload is a worse outcome than one scene coming out flat.
fn upload_sheet(ctx: &mut VulkanContext) -> Option<VulkanTexture> {
    let mut texture = match ctx.create_texture(&TextureDescriptor::offscreen(
        Extent2D::new(4, 4),
        PixelFormat::Rgba8Unorm,
    )) {
        Ok(texture) => texture,
        Err(e) => {
            eprintln!("cannot allocate the sprite sheet: {e}");
            return None;
        }
    };
    match ctx.write_texture(&mut texture, &live::sheet_pixels()) {
        Ok(()) => Some(texture),
        Err(e) => {
            eprintln!("cannot upload the sprite sheet: {e}");
            ctx.destroy_texture(texture);
            None
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.stage.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("emblema playground")
            .with_inner_size(winit::dpi::LogicalSize::new(INITIAL, INITIAL));
        let window = event_loop.create_window(attributes).expect("window");

        let mut ctx = VulkanContext::new(DevicePreference::Auto).expect("vulkan context");
        // The instance already asks for the platform surface extensions; this
        // is the caller bringing a surface, which is the arrangement the
        // presentation crate is built around.
        let surface = unsafe {
            ash_window::create_surface(
                ctx.raw_entry(),
                ctx.raw_instance(),
                window.display_handle().expect("display handle").as_raw(),
                window.window_handle().expect("window handle").as_raw(),
                None,
            )
        }
        .expect("surface");

        let size = window.inner_size();
        let extent = Extent2D::new(size.width.max(1), size.height.max(1));
        let target =
            SwapchainTarget::new(&mut ctx, surface, extent, PresentMode::Fifo).expect("swapchain");

        eprintln!(
            "{}\n{} fixed scenes and {} live ones.\n\
             Right/Left or Space to step, Up/Down to turn the knob, A to animate, Q to quit.",
            ctx.capabilities().device_name,
            self.scenes.len(),
            self.live.len()
        );

        let sheet = upload_sheet(&mut ctx);

        self.stage = Some(Stage {
            window,
            ctx,
            target,
            surface,
            scene_texture: None,
            sheet,
        });
        self.announce();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(stage) = &mut self.stage {
                    let extent = Extent2D::new(size.width.max(1), size.height.max(1));
                    if let Err(e) = stage.target.reconfigure(&mut stage.ctx, extent) {
                        eprintln!("reconfigure: {e}");
                    }
                    stage.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::ArrowRight) | Key::Named(NamedKey::Space) => {
                        self.step(true)
                    }
                    Key::Named(NamedKey::ArrowLeft) => self.step(false),
                    Key::Named(NamedKey::ArrowUp) => self.turn(0.05),
                    Key::Named(NamedKey::ArrowDown) => self.turn(-0.05),
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Character(ref c) if c.as_str() == "q" => event_loop.exit(),
                    Key::Character(ref c) if c.as_str() == "a" => {
                        self.animating = !self.animating;
                        self.announce();
                        self.request_redraw();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        // A moving scene asks for the next frame itself; a still one waits for
        // an event, so the tool costs nothing while it sits there.
        if self.is_moving() {
            self.request_redraw();
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        // Torn down in the order the objects depend on each other: the scene
        // texture and the swapchain belong to the context, and the surface
        // outlives the swapchain built on it.
        if let Some(mut stage) = self.stage.take() {
            stage.drop_scene_texture();
            let Stage {
                mut ctx,
                target,
                surface,
                ..
            } = stage;
            target.destroy(&mut ctx);
            // SAFETY: the swapchain built on it has just been destroyed.
            unsafe { emblema::wsi::destroy_surface(&ctx, surface) };
        }
    }
}
