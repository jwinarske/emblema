//! What a display controller made of the buffer this renderer exported.
//!
//! Every other test on this path asserts that an export was *accepted*: that a
//! dma-buf became a framebuffer, that an atomic commit carrying it was valid,
//! that a flip event came back. None of them reads a pixel, so a kernel that
//! took the agreed modifier or fourcc to mean something other than the renderer
//! did would pass all of them with a scrambled picture on screen. `negotiate`
//! agreeing a layout is not evidence that both sides mean the same thing by it.
//!
//! A writeback connector is the controller's own answer. The CRTC composites
//! the plane and writes the result into a buffer this process can map, so the
//! picture can be compared against what the renderer drew.
//!
//! # The instrument lives here
//!
//! None of this is in `src/`. Writeback is how the tree checks itself rather
//! than something a consumer presents through, and this crate is published —
//! the same reason `scanout.rs` keeps its `ScanoutOutput` stand-in in the test.
//! The cost is that the plane and property lookups below restate simpler
//! versions of `kms.rs`'s; if a third caller ever wants them, that is when they
//! should move.
//!
//! # Not disturbing a display
//!
//! This sets a mode, where `kms.rs` only flips onto one already configured, so
//! it is stricter about what it will drive. Unless `EMBLEMA_DRM_CARD` names a
//! card the driver must be `vkms`, read from the kernel through `get_driver`
//! rather than from sysfs — on this kernel vkms sits on the faux bus and sysfs
//! reports its driver as `faux_driver`. Master must also be acquired, which the
//! kernel refuses while anything else holds it, and which is what has always
//! kept `kms.rs` off a display it should not touch.
//!
//! # Why the software render device
//!
//! Rendering through whatever device the machine picks was tried first and
//! found a disagreement that is not the renderer's: on RADV here, the kernel's
//! read of the exported buffer is the frame only in its last stretch, while a
//! Vulkan readback of the same image is the frame throughout.
//! `docs/on-a-board.md` holds the figures and what is ruled out. Taking the
//! software device keeps this sensitive to what it is for -- a fourcc, a stride
//! or an offset meant differently by the two sides -- and does not pretend that
//! one is settled.
//!
//! What no device here can cover is a non-linear modifier, because nothing in
//! this fleet scans one out: vkms takes linear alone, and so does the vc4
//! controller a Raspberry Pi offers.

// Reached from a helper rather than from a test body, so clippy's test-code
// exemption does not see it, as in `scanout.rs`. A misspelled
// `EMBLEMA_WRITEBACK_DEVICE` has to stop the run: falling back to the default
// would let a board session report a pass for a comparison it never made.
#![allow(clippy::panic)]

use drm::buffer::Buffer as _;
use drm::control::{self, Device as _};
use drm::Device as _;
use emblema_hal::{
    Batch, BlendMode, Extent2D, Fourcc, Material, Modifier, PassDescriptor, PixelFormat,
    TextureDescriptor,
};
use emblema_hal_vulkan::{DevicePreference, Validated};
use emblema_present::negotiate::negotiate;
use emblema_present_drm::device::DrmDevice;
use emblema_testkit::{accepts, compare, Image, Tolerance};
use std::collections::HashMap;

const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// Quadrant corners in clip space, and the color each is filled with.
///
/// The colors are the whole reason this is not one flat quad. A flat fill is
/// invariant under a misread tiling, a transposed readback and a channel swap,
/// so it would pass against all three. These have no two channels equal within
/// a color and no color a channel permutation of another, which makes the
/// palette itself sensitive to a swap.
///
/// Each lands on a whole byte: 0.250_980_4 * 255 is 64, not 63.9.
const QUADRANTS: [([f32; 2], [f32; 2], [f32; 4]); 4] = [
    ([-1.0, -1.0], [0.0, 0.0], [1.0, 0.250_980_4, 0.0, 1.0]),
    ([0.0, -1.0], [1.0, 0.0], [0.0, 0.501_960_8, 1.0, 1.0]),
    (
        [-1.0, 0.0],
        [0.0, 1.0],
        [0.125_490_2, 1.0, 0.376_470_6, 1.0],
    ),
    ([0.0, 0.0], [1.0, 1.0], [0.752_941_2, 0.0, 0.627_451, 1.0]),
];

/// A band corner to corner, so an error the size of a tile shows up as a break
/// in a line rather than as a seam between two flat colors.
const DIAGONAL: [f32; 4] = [0.941_176_5, 0.815_686_3, 0.250_980_4, 1.0];

/// The byte each quadrant color is expected to read back as, after the swizzle.
///
/// Asserted by presence rather than by position, which is what makes it
/// independent of how clip space maps to rows — and still enough to catch a
/// channel swap, since no entry is a permutation of another.
const EXPECTED: [[u8; 4]; 4] = [
    [255, 64, 0, 255],
    [0, 128, 255, 255],
    [32, 255, 96, 255],
    [192, 0, 160, 255],
];

mod common;

fn scene() -> Batch {
    let mut batch = Batch::new();
    for (lo, hi, color) in QUADRANTS {
        let corners = [
            [lo[0], lo[1]],
            [hi[0], lo[1]],
            [hi[0], hi[1]],
            [lo[0], hi[1]],
        ];
        batch
            .push(&corners, &QUAD, Material::solid(color), BlendMode::Src)
            .expect("push a quadrant");
    }
    const W: f32 = 0.12;
    let band = [
        [-1.0, -1.0 + W],
        [-1.0 + W, -1.0],
        [1.0, 1.0 - W],
        [1.0 - W, 1.0],
    ];
    batch
        .push(&band, &QUAD, Material::solid(DIAGONAL), BlendMode::Src)
        .expect("push the diagonal");
    batch
}

/// Bytes the kernel or the renderer produced, as an `Image`.
///
/// Both sides are BGRA — `Bgra8Unorm` on one and a little-endian `ARGB8888` or
/// `XRGB8888` on the other — so both go through this, and alpha is forced
/// because an `X` fourcc leaves that byte undefined. A mistake here therefore
/// cancels in the comparison, which is what `a_quadrant_reads_back_the_color_it_was_drawn`
/// is for.
fn as_image(bytes: &[u8], extent: Extent2D, pitch: u32) -> Image {
    let mut pixels = Vec::with_capacity((extent.width * extent.height * 4) as usize);
    for y in 0..extent.height {
        let row = (y * pitch) as usize;
        for x in 0..extent.width {
            let p = row + (x * 4) as usize;
            pixels.extend_from_slice(&[bytes[p + 2], bytes[p + 1], bytes[p], 255]);
        }
    }
    Image::new(extent.width, extent.height, pixels)
}

/// The render device, software unless a run asks for the machine's own.
///
/// Software by default for the reason the module doc gives. `EMBLEMA_WRITEBACK_DEVICE=auto`
/// is what a board session sets: on hardware with a display controller that
/// composites by DMA rather than with the CPU, the question the default is
/// avoiding does not arise, and a real export is the stronger thing to check.
fn preference() -> DevicePreference {
    match std::env::var("EMBLEMA_WRITEBACK_DEVICE").as_deref() {
        Ok("auto") => DevicePreference::Auto,
        Ok("software") | Err(_) => DevicePreference::Software,
        Ok(other) => panic!("EMBLEMA_WRITEBACK_DEVICE={other}, which is neither auto nor software"),
    }
}

fn context() -> Option<Validated> {
    match Validated::new(preference()) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no Vulkan device ({e})");
            None
        }
    }
}

/// Every property of one object, by name, with the value the kernel reports.
fn properties<T: control::ResourceHandle>(
    device: &DrmDevice,
    handle: T,
) -> HashMap<String, (control::property::Handle, u64)> {
    let mut out = HashMap::new();
    let Ok(set) = device.get_properties(handle) else {
        return out;
    };
    let (ids, values) = set.as_props_and_values();
    for (id, value) in ids.iter().zip(values) {
        if let Ok(info) = device.get_property(*id) {
            out.insert(info.name().to_string_lossy().into_owned(), (*id, *value));
        }
    }
    out
}

/// A plane that can drive this CRTC and is a primary rather than an overlay.
///
/// `possible_crtcs` is not decoration; `kms.rs::primary_plane_for` carries the
/// story of what omitting it cost on a Pi 5.
fn primary_plane(
    device: &DrmDevice,
    resources: &control::ResourceHandles,
    crtc: control::crtc::Handle,
) -> Option<control::plane::Handle> {
    device.plane_handles().ok()?.into_iter().find(|handle| {
        let Ok(info) = device.get_plane(*handle) else {
            return false;
        };
        if info.crtc().is_some_and(|bound| bound != crtc) {
            return false;
        }
        if !resources
            .filter_crtcs(info.possible_crtcs())
            .contains(&crtc)
        {
            return false;
        }
        properties(device, *handle)
            .get("type")
            .is_some_and(|(_, value)| *value == 1)
    })
}

/// The pieces a writeback commit needs, or the reason there are none.
struct Writeback {
    device: DrmDevice,
    connector: control::connector::Handle,
    crtc: control::crtc::Handle,
    plane: control::plane::Handle,
    mode: control::Mode,
    /// What the destination buffer will be, chosen from
    /// `WRITEBACK_PIXEL_FORMATS` rather than assumed.
    fourcc: Fourcc,
    connector_props: HashMap<String, (control::property::Handle, u64)>,
    crtc_props: HashMap<String, (control::property::Handle, u64)>,
    plane_props: HashMap<String, (control::property::Handle, u64)>,
}

impl Writeback {
    fn find() -> Option<(Self, common::CardGuard)> {
        let guard = common::take_the_card();
        let mut refused = Vec::new();

        let named = std::env::var("EMBLEMA_DRM_CARD").ok();
        let paths: Vec<String> = match &named {
            Some(path) => vec![path.clone()],
            None => {
                let mut paths: Vec<String> = std::fs::read_dir("/dev/dri")
                    .ok()?
                    .flatten()
                    .map(|e| e.path().to_string_lossy().into_owned())
                    .filter(|p| p.contains("/card"))
                    .collect();
                paths.sort();
                paths
            }
        };

        for path in paths {
            match Self::open(&path, named.is_some()) {
                Ok(found) => return Some((found, guard)),
                Err(reason) => refused.push(format!("{path}: {reason}")),
            }
        }
        eprintln!(
            "skipping: no card offers writeback ({})",
            refused.join("; ")
        );
        None
    }

    fn open(path: &str, named: bool) -> std::result::Result<Self, String> {
        let device = DrmDevice::open(path).map_err(|e| e.to_string())?;

        // The kernel's own name for the driver, which is `vkms` whichever bus
        // the device hangs off.
        let driver = device
            .get_driver()
            .map_err(|e| format!("get_driver: {e}"))?
            .name()
            .to_string_lossy()
            .into_owned();
        if !named && driver != "vkms" {
            return Err(format!(
                "{driver} was not named by EMBLEMA_DRM_CARD, and only vkms is driven unasked"
            ));
        }

        // Before anything is enumerated, because a writeback connector is
        // hidden from a client that has not asked to see it -- the same shape as
        // the universal-planes capability `DrmDevice::open` sets, and with the
        // same symptom, a card that appears to have no writeback connector at
        // all. The capability needs atomic, which `become_master` turns on, so
        // this is also what fixes the order: master comes before the survey
        // rather than after it.
        device.become_master().map_err(|e| e.to_string())?;
        device
            .set_client_capability(drm::ClientCapability::WritebackConnectors, true)
            .map_err(|e| format!("enabling writeback connectors: {e}"))?;

        let resources = device
            .resource_handles()
            .map_err(|e| format!("resource_handles: {e}"))?;

        // A writeback connector reports no modes of its own, so the mode comes
        // from a connector on this card that has one.
        let mut writeback = None;
        let mut mode = None;
        for handle in resources.connectors() {
            let Ok(info) = device.get_connector(*handle, false) else {
                continue;
            };
            if info.interface() == control::connector::Interface::Writeback {
                writeback = writeback.or(Some(info));
            } else if mode.is_none() {
                mode = info.modes().first().copied();
            }
        }
        let connector = writeback.ok_or("no writeback connector")?;
        let mode = mode.ok_or("no connector on this card reports a mode to borrow")?;

        let connector_props = properties(&device, connector.handle());
        for name in [
            "WRITEBACK_FB_ID",
            "WRITEBACK_OUT_FENCE_PTR",
            "WRITEBACK_PIXEL_FORMATS",
            "CRTC_ID",
        ] {
            if !connector_props.contains_key(name) {
                return Err(format!("the writeback connector has no {name}"));
            }
        }

        let fourcc = writeback_format(&device, &connector_props)?;

        // A CRTC the writeback connector's encoder can be driven from, with a
        // plane to put the frame on.
        //
        // Refusing a CRTC the kernel reports `ACTIVE` was tried here and is not
        // the guard it sounds like: fbcon binds to vkms, so the one CRTC vkms
        // has is active at rest and the rule excluded the only card that works.
        // What keeps this off a display is the driver check above and the master
        // lock, which is what `kms.rs` has always relied on while setting a mode
        // on this same CRTC.
        let mut chosen = None;
        for encoder in connector.encoders() {
            let Ok(info) = device.get_encoder(*encoder) else {
                continue;
            };
            for crtc in resources.filter_crtcs(info.possible_crtcs()) {
                if let Some(plane) = primary_plane(&device, &resources, crtc) {
                    chosen = Some((crtc, plane, properties(&device, crtc)));
                    break;
                }
            }
            if chosen.is_some() {
                break;
            }
        }
        let (crtc, plane, crtc_props) = chosen
            .ok_or("no CRTC the writeback connector can be driven from has a primary plane")?;

        Ok(Self {
            plane_props: properties(&device, plane),
            device,
            connector: connector.handle(),
            crtc,
            plane,
            mode,
            fourcc,
            connector_props,
            crtc_props,
        })
    }

    fn extent(&self) -> Extent2D {
        let (width, height) = self.mode.size();
        Extent2D::new(width as u32, height as u32)
    }
}

/// A destination format both the connector and this test can read.
///
/// `WRITEBACK_PIXEL_FORMATS` is a flat array of fourccs, which is what makes it
/// so much simpler than `IN_FORMATS`.
fn writeback_format(
    device: &DrmDevice,
    props: &HashMap<String, (control::property::Handle, u64)>,
) -> std::result::Result<Fourcc, String> {
    let (_, blob) = props["WRITEBACK_PIXEL_FORMATS"];
    let bytes = device
        .get_property_blob(blob)
        .map_err(|e| format!("WRITEBACK_PIXEL_FORMATS blob: {e}"))?;
    let advertised: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    [Fourcc::XRGB8888, Fourcc::ARGB8888]
        .into_iter()
        .find(|want| advertised.contains(&want.0))
        .ok_or_else(|| {
            format!("writeback advertises none of XRGB8888 or ARGB8888, only {advertised:?}")
        })
}

/// Draw the scene, hand the export to the controller, and read back what it
/// composited.
fn composite(wb: &mut Writeback, ctx: &mut Validated) -> std::result::Result<Image, String> {
    let extent = wb.extent();

    // Linear is a valid answer: vkms accepts nothing else, and the point is
    // that it was reported rather than assumed.
    let scanout: Vec<_> = {
        let plane_formats = emblema_present_drm::device::parse_in_formats(
            &wb.device
                .get_property_blob(wb.plane_props["IN_FORMATS"].1)
                .map_err(|e| format!("IN_FORMATS blob: {e}"))?,
        );
        plane_formats
    };
    let agreed = negotiate(
        &ctx.capabilities().render_formats.clone(),
        &scanout,
        &[Fourcc::ARGB8888, Fourcc::XRGB8888],
    )
    .map_err(|e| format!("the render device and this plane share no layout: {e}"))?;
    eprintln!(
        "{} on {}: {}x{}, source {:?} {:?}, writeback {:?}",
        wb.device.path(),
        ctx.capabilities().device_name,
        extent.width,
        extent.height,
        agreed.fourcc,
        agreed.modifier,
        wb.fourcc
    );

    let mut source = ctx
        .create_exportable_texture(extent, PixelFormat::Bgra8Unorm, &[agreed.modifier])
        .map_err(|e| format!("exportable image: {e}"))?;
    let batch = scene();
    ctx.submit_batch(
        &mut source,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
    )
    .map_err(|e| format!("render into the scanout buffer: {e}"))?;
    let exported = ctx
        .export_texture(&source)
        .map_err(|e| format!("dma-buf export: {e}"))?;

    let mut pitches = [0u32; 4];
    let mut offsets = [0u32; 4];
    let mut handles = [None; 4];
    let mut owned = Vec::new();
    for (i, plane) in exported.planes.iter().enumerate().take(4) {
        let handle = wb
            .device
            .prime_fd_to_buffer(std::os::fd::AsFd::as_fd(&plane.fd))
            .map_err(|e| format!("prime_fd_to_buffer: {e}"))?;
        handles[i] = Some(handle);
        owned.push(handle);
        pitches[i] = plane.stride;
        offsets[i] = plane.offset;
    }
    let source_fb = wb
        .device
        .add_planar_framebuffer(
            &Planar {
                extent,
                fourcc: agreed.fourcc,
                modifier: Some(agreed.modifier),
                pitches,
                offsets,
                handles,
            },
            control::FbCmd2Flags::MODIFIERS,
        )
        .map_err(|e| format!("the display controller refused the exported buffer: {e}"))?;

    // The destination. A dumb buffer because it has to be mapped, which is the
    // one thing a dma-buf from the render device does not make easy.
    let mut dumb = wb
        .device
        .create_dumb_buffer(
            (extent.width, extent.height),
            drm::buffer::DrmFourcc::Xrgb8888,
            32,
        )
        .map_err(|e| format!("create_dumb_buffer: {e}"))?;
    let pitch = dumb.pitch();
    let dest_fb = wb
        .device
        .add_planar_framebuffer(
            &Planar {
                extent,
                fourcc: wb.fourcc,
                modifier: None,
                pitches: [pitch, 0, 0, 0],
                offsets: [0; 4],
                handles: [Some(dumb.handle()), None, None, None],
            },
            control::FbCmd2Flags::empty(),
        )
        .map_err(|e| format!("the writeback destination was refused: {e}"))?;

    let mode_blob = wb
        .device
        .create_property_blob(&wb.mode)
        .map_err(|e| format!("create_property_blob: {e}"))?;

    // Where the kernel writes the fd for the fence that says the job is done.
    // A pointer rather than a value, which is why this goes in as a raw u64.
    let mut fence: i32 = -1;

    let mut atomic = control::atomic::AtomicModeReq::new();
    let connector = wb.connector.into();
    let crtc = wb.crtc.into();
    let plane = wb.plane.into();
    let mut set = |object,
                   props: &HashMap<String, (control::property::Handle, u64)>,
                   name: &str,
                   value: u64| {
        atomic.add_raw_property(object, props[name].0, value);
    };
    set(
        connector,
        &wb.connector_props,
        "CRTC_ID",
        u32::from(wb.crtc) as u64,
    );
    set(
        connector,
        &wb.connector_props,
        "WRITEBACK_FB_ID",
        u32::from(dest_fb) as u64,
    );
    set(
        connector,
        &wb.connector_props,
        "WRITEBACK_OUT_FENCE_PTR",
        &mut fence as *mut i32 as u64,
    );
    set(crtc, &wb.crtc_props, "ACTIVE", 1);
    set(plane, &wb.plane_props, "FB_ID", u32::from(source_fb) as u64);
    set(plane, &wb.plane_props, "CRTC_ID", u32::from(wb.crtc) as u64);
    // Source in 16.16 fixed point, destination in whole pixels. Mixing the two
    // up scales the frame by 65536.
    set(plane, &wb.plane_props, "SRC_X", 0);
    set(plane, &wb.plane_props, "SRC_Y", 0);
    set(plane, &wb.plane_props, "SRC_W", (extent.width as u64) << 16);
    set(
        plane,
        &wb.plane_props,
        "SRC_H",
        (extent.height as u64) << 16,
    );
    set(plane, &wb.plane_props, "CRTC_X", 0);
    set(plane, &wb.plane_props, "CRTC_Y", 0);
    set(plane, &wb.plane_props, "CRTC_W", extent.width as u64);
    set(plane, &wb.plane_props, "CRTC_H", extent.height as u64);
    // The blob goes in as a typed value, since that is what carries its id.
    atomic.add_property(wb.crtc, wb.crtc_props["MODE_ID"].0, mode_blob);

    // Blocking and allowed to modeset: one commit that enables the CRTC, puts
    // the exported buffer on the plane and asks for the result back.
    let committed = wb
        .device
        .atomic_commit(control::AtomicCommitFlags::ALLOW_MODESET, atomic)
        .map_err(|e| format!("atomic_commit: {e}"));

    let image = committed.and_then(|()| {
        wait_for_fence(fence)?;
        let mapping = wb
            .device
            .map_dumb_buffer(&mut dumb)
            .map_err(|e| format!("map_dumb_buffer: {e}"))?;
        Ok(as_image(&mapping, extent, pitch))
    });

    let _ = wb.device.destroy_framebuffer(dest_fb);
    let _ = wb.device.destroy_framebuffer(source_fb);
    let _ = wb.device.destroy_dumb_buffer(dumb);
    for handle in owned {
        let _ = wb.device.close_buffer(handle);
    }
    ctx.destroy_texture(source);
    image
}

/// Wait for the writeback job, through the fence the commit handed back.
///
/// A `sync_file` polls readable when it signals, which is the only thing that
/// says the composition is finished — the commit returning says the request was
/// accepted.
fn wait_for_fence(fd: i32) -> std::result::Result<(), String> {
    use std::os::fd::FromRawFd as _;
    if fd < 0 {
        return Err("the commit returned no writeback fence".into());
    }
    // SAFETY: the kernel wrote this fd into `fence` for this commit and nothing
    // else holds it.
    let fence = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
    let mut fds = [rustix::event::PollFd::new(
        &fence,
        rustix::event::PollFlags::IN,
    )];
    // Fifteen seconds rather than a frame: at the mode vkms offers first the
    // fence signals immediately, but at 2560x1600 it took longer than two, and
    // a timeout tuned on the small mode reads as a hang on a large one.
    let timeout = rustix::event::Timespec {
        tv_sec: 15,
        tv_nsec: 0,
    };
    match rustix::event::poll(&mut fds, Some(&timeout)) {
        Ok(0) => Err("the writeback fence did not signal in time".into()),
        Ok(_) => Ok(()),
        Err(e) => Err(format!("poll on the writeback fence: {e}")),
    }
}

/// The renderer's own picture of the same batch, through an ordinary offscreen
/// target: the thing the controller's composition is compared against.
fn offscreen(ctx: &mut Validated, extent: Extent2D) -> std::result::Result<Image, String> {
    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(
            extent,
            PixelFormat::Bgra8Unorm,
        ))
        .map_err(|e| format!("offscreen target: {e}"))?;
    let batch = scene();
    ctx.submit_batch(
        &mut target,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
    )
    .map_err(|e| format!("render offscreen: {e}"))?;
    let bytes = ctx
        .read_texture(&mut target)
        .map_err(|e| format!("read back the offscreen render: {e}"))?;
    let image = as_image(&bytes, extent, extent.width * 4);
    ctx.destroy_texture(target);
    Ok(image)
}

/// A `PlanarBuffer` over handles this test already owns.
struct Planar {
    extent: Extent2D,
    fourcc: Fourcc,
    modifier: Option<Modifier>,
    pitches: [u32; 4],
    offsets: [u32; 4],
    handles: [Option<drm::buffer::Handle>; 4],
}

impl drm::buffer::PlanarBuffer for Planar {
    fn size(&self) -> (u32, u32) {
        (self.extent.width, self.extent.height)
    }
    fn format(&self) -> drm::buffer::DrmFourcc {
        drm::buffer::DrmFourcc::try_from(self.fourcc.0).unwrap_or(drm::buffer::DrmFourcc::Xrgb8888)
    }
    fn modifier(&self) -> Option<drm::buffer::DrmModifier> {
        self.modifier.map(|m| drm::buffer::DrmModifier::from(m.0))
    }
    fn pitches(&self) -> [u32; 4] {
        self.pitches
    }
    fn handles(&self) -> [Option<drm::buffer::Handle>; 4] {
        self.handles
    }
    fn offsets(&self) -> [u32; 4] {
        self.offsets
    }
}

/// Everything the capture has to be for the comparison below to mean anything.
///
/// A zeroed dumb buffer, a capture of the wrong CRTC and a commit the kernel
/// quietly dropped all produce one flat color, and the comparison alone would
/// not tell those apart from a disagreement.
#[test]
fn the_capture_holds_a_picture_and_not_one_flat_color() {
    let (Some((mut wb, _card)), Some(mut ctx)) = (Writeback::find(), context()) else {
        return;
    };
    let image = match composite(&mut wb, &mut ctx) {
        Ok(image) => image,
        Err(e) => panic!("{e}"),
    };
    let first = image.pixel(0, 0);
    let distinct = (0..image.height)
        .step_by(7)
        .flat_map(|y| (0..image.width).step_by(7).map(move |x| (x, y)))
        .map(|(x, y)| image.pixel(x, y))
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        distinct.len() >= 5,
        "the capture holds {} distinct colors, starting {first:?}; the scene draws five regions",
        distinct.len()
    );
}

/// The one assertion the comparison cannot make.
///
/// Both sides go through the same swizzle, so a channel order read wrong on
/// both cancels. This is ground truth: a color drawn pure is expected back as
/// the bytes it was drawn as, and no two of them are permutations, so a swap
/// cannot hide.
#[test]
fn a_quadrant_reads_back_the_color_it_was_drawn() {
    let (Some((mut wb, _card)), Some(mut ctx)) = (Writeback::find(), context()) else {
        return;
    };
    let image = match composite(&mut wb, &mut ctx) {
        Ok(image) => image,
        Err(e) => panic!("{e}"),
    };
    for want in EXPECTED {
        let found = (0..image.height).any(|y| {
            (0..image.width).any(|x| {
                let got = image.pixel(x, y);
                (0..4).all(|c| got[c].abs_diff(want[c]) <= 1)
            })
        });
        assert!(
            found,
            "nothing in the capture is {want:?}, which one quadrant was filled with"
        );
    }
}

/// The comparison this file exists for.
#[test]
fn the_controller_composited_what_the_renderer_drew() {
    let (Some((mut wb, _card)), Some(mut ctx)) = (Writeback::find(), context()) else {
        return;
    };
    let extent = wb.extent();
    let captured = match composite(&mut wb, &mut ctx) {
        Ok(image) => image,
        Err(e) => panic!("{e}"),
    };
    let drawn = match offscreen(&mut ctx, extent) {
        Ok(image) => image,
        Err(e) => panic!("{e}"),
    };

    let difference = compare(&captured, &drawn).expect("two images of the same size");
    // vkms composites through a wider intermediate, so a byte either way is the
    // honest allowance. Anything past it is the finding, not the noise.
    let tolerance = Tolerance::ROUNDING;
    assert!(
        accepts(&difference, tolerance),
        "the controller's composition differs from the render: {}",
        difference.describe(tolerance)
    );
}
