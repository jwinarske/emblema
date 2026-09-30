//! A [`ScanoutOutput`] backed by a real KMS device.
//!
//! The half of the scanout path that needs modesetting master: turning an
//! exported buffer into a framebuffer, committing it atomically, and reading
//! the events that say when a flip landed.
//!
//! # Why this could not be written until now
//!
//! Only one process may be master of a card, and a compositor holds it on any
//! card driving a display. Writing this against no device would have meant
//! untested modesetting code, which is the thing the testing model exists to
//! rule out. The virtual KMS driver gives a real device — atomic commits,
//! vblank, a writeback connector — with no display behind it, so this is
//! written against something that answers.
//!
//! [`ScanoutOutput`]: crate::output::ScanoutOutput

use crate::device::DrmDevice;
use crate::output::{CommitRequest, DmaBufPlanes, OutputEvent, ScanoutOutput};
use crate::{FbHandle, Mode};
use drm::control::{self, Device as _};
use emblema_hal::{Error, Extent2D, FormatModifierSet, Result};
use std::collections::HashMap;

/// The pieces of a display pipeline a frame has to be committed against.
///
/// Chosen once and held, because they are what the mode was set for: picking a
/// different CRTC per frame would mean a modeset per frame.
struct Pipeline {
    connector: control::connector::Handle,
    crtc: control::crtc::Handle,
    plane: control::plane::Handle,
    mode: control::Mode,
}

/// Property handles, looked up once by name.
///
/// The kernel's property IDs are per device and per object type, so every one
/// of these is a lookup rather than a constant. Doing it once at construction
/// is what keeps a commit from being a dozen string comparisons.
struct Properties {
    connector_crtc_id: control::property::Handle,
    crtc_mode_id: control::property::Handle,
    crtc_active: control::property::Handle,
    plane: HashMap<&'static str, control::property::Handle>,
    /// Where the render-done fence is attached, if this driver takes one.
    ///
    /// Optional: a driver without it needs the caller to have waited on the
    /// CPU, which is the fallback the commit request documents.
    plane_in_fence_fd: Option<control::property::Handle>,
}

/// A KMS device driving one connector.
pub struct KmsOutput {
    device: DrmDevice,
    pipeline: Pipeline,
    properties: Properties,
    /// The mode as a property blob, created once because a modeset needs one
    /// and creating it per commit would leak a blob per frame.
    mode_blob: control::property::Value<'static>,
    formats: Vec<FormatModifierSet>,
    /// Framebuffers this output created, by the handle it handed out.
    ///
    /// Kept so that releasing one can find the GEM handles to close alongside
    /// it: a framebuffer holds a reference to its buffer objects, and dropping
    /// only the framebuffer leaks them.
    framebuffers: HashMap<u64, Framebuffer>,
    /// Committed and not yet reported as on screen.
    in_flight: Option<FbHandle>,
    /// Which vertical blanks the flips landed on; see [`crate::pacing`].
    pacing: crate::pacing::Pacing,
    pending: Vec<OutputEvent>,
}

struct Framebuffer {
    fb: control::framebuffer::Handle,
    buffers: Vec<drm::buffer::Handle>,
}

impl KmsOutput {
    /// Take a card and drive its first connected connector.
    ///
    /// Becomes master, which fails where something else already is. That is a
    /// normal outcome rather than a defect here: the answer is another device.
    pub fn open(path: &str) -> Result<Self> {
        let device = DrmDevice::open(path)?;
        device.become_master()?;

        let resources = device
            .resource_handles()
            .map_err(|e| err("resource_handles", e))?;

        // A connected connector with at least one mode. Anything else has
        // nothing to display and nothing to display it at.
        let (connector, mode) = resources
            .connectors()
            .iter()
            .filter_map(|handle| device.get_connector(*handle, false).ok())
            .filter(|c| c.state() == control::connector::State::Connected)
            .find_map(|c| c.modes().first().map(|m| (c.handle(), *m)))
            .ok_or(Error::Unsupported(
                "no connected connector with a mode to display at",
            ))?;

        // The encoder a connector is attached to says which CRTCs can drive it.
        // Taking the first compatible one is enough for a single output; a
        // second display would need them allocated rather than chosen.
        let connector_info = device
            .get_connector(connector, false)
            .map_err(|e| err("get_connector", e))?;
        let crtc = connector_info
            .current_encoder()
            .and_then(|e| device.get_encoder(e).ok())
            .and_then(|e| e.crtc())
            .or_else(|| resources.crtcs().first().copied())
            .ok_or(Error::Unsupported("no CRTC to drive this connector"))?;

        let plane = primary_plane_for(&device, &resources, crtc)?;
        let formats = plane_formats(&device, plane)?;
        let properties = Properties::read(&device, connector, crtc, plane)?;

        let mode_blob = device
            .create_property_blob(&mode)
            .map_err(|e| err("create_property_blob", e))?;

        Ok(Self {
            device,
            pipeline: Pipeline {
                connector,
                crtc,
                plane,
                mode,
            },
            properties,
            mode_blob,
            formats,
            framebuffers: HashMap::new(),
            in_flight: None,
            pacing: crate::pacing::Pacing::new(),
            pending: Vec::new(),
        })
    }

    pub fn path(&self) -> &str {
        self.device.path()
    }
}

/// A primary plane that can drive this CRTC.
///
/// Three conditions, and the third used to be missing. The plane must not
/// already be bound to a different CRTC, it must be a primary rather than an
/// overlay or a cursor, and the kernel must say it can drive *this* CRTC.
///
/// That last one is `possible_crtcs`, and it is not decoration. A Raspberry Pi
/// 5's `vc4` carries four CRTCs and forty-eight planes, and its planes report a
/// mask of `1110` — every CRTC except the first, which is the one a single
/// connected output is otherwise given. Committing a plane to a CRTC it cannot
/// drive fails the whole atomic request with `EINVAL`, which is what it did:
/// every frame, on the HDMI output, with nothing in the kernel log to say why.
///
/// The comment that stood here said the mask "would be the precise answer" and
/// that drm-rs "wraps it in a newtype with no accessor". Half right. The raw
/// bits are indeed private, and `ResourceHandles::filter_crtcs` applies the
/// mask and hands back the handles — which is the question being asked, rather
/// than the bits it is answered from.
fn primary_plane_for(
    device: &DrmDevice,
    resources: &control::ResourceHandles,
    crtc: control::crtc::Handle,
) -> Result<control::plane::Handle> {
    for handle in device
        .plane_handles()
        .map_err(|e| err("plane_handles", e))?
    {
        let Ok(info) = device.get_plane(handle) else {
            continue;
        };
        if info.crtc().is_some_and(|bound| bound != crtc) {
            continue;
        }
        if !resources
            .filter_crtcs(info.possible_crtcs())
            .contains(&crtc)
        {
            continue;
        }
        if is_primary(device, handle) {
            return Ok(handle);
        }
    }
    Err(Error::Unsupported(
        "no primary plane can drive the chosen CRTC",
    ))
}

fn is_primary(device: &DrmDevice, plane: control::plane::Handle) -> bool {
    let Ok(properties) = device.get_properties(plane) else {
        return false;
    };
    // Slices rather than the iterator, because the iterator borrows the set
    // through a temporary that does not outlive the expression.
    let (ids, values) = properties.as_props_and_values();
    ids.iter().zip(values).any(|(id, value)| {
        device
            .get_property(*id)
            .is_ok_and(|info| info.name().to_str() == Ok("type") && *value == 1)
    })
}

fn plane_formats(
    device: &DrmDevice,
    plane: control::plane::Handle,
) -> Result<Vec<FormatModifierSet>> {
    let properties = device
        .get_properties(plane)
        .map_err(|e| err("get_properties", e))?;
    let (ids, values) = properties.as_props_and_values();
    for (id, value) in ids.iter().zip(values) {
        let Ok(info) = device.get_property(*id) else {
            continue;
        };
        if info.name().to_str() == Ok("IN_FORMATS") {
            if let Ok(blob) = device.get_property_blob(*value) {
                return Ok(crate::device::parse_in_formats(&blob));
            }
        }
    }
    Ok(Vec::new())
}

impl Properties {
    fn read(
        device: &DrmDevice,
        connector: control::connector::Handle,
        crtc: control::crtc::Handle,
        plane: control::plane::Handle,
    ) -> Result<Self> {
        let find = |handle: control::property::Handle| -> Result<String> {
            let info = device
                .get_property(handle)
                .map_err(|e| err("property", e))?;
            Ok(info.name().to_string_lossy().into_owned())
        };
        let collect = |object: &dyn Fn() -> Result<control::PropertyValueSet>| -> Result<
            HashMap<String, control::property::Handle>,
        > {
            let set = object()?;
            let mut out = HashMap::new();
            for id in set.as_props_and_values().0 {
                out.insert(find(*id)?, *id);
            }
            Ok(out)
        };

        let connector_props = collect(&|| {
            device
                .get_properties(connector)
                .map_err(|e| err("connector properties", e))
        })?;
        let crtc_props = collect(&|| {
            device
                .get_properties(crtc)
                .map_err(|e| err("crtc properties", e))
        })?;
        let plane_props = collect(&|| {
            device
                .get_properties(plane)
                .map_err(|e| err("plane properties", e))
        })?;

        let need = |set: &HashMap<String, control::property::Handle>,
                    name: &str|
         -> Result<control::property::Handle> {
            set.get(name).copied().ok_or(Error::Unsupported(
                "this device is missing an atomic property the commit path needs",
            ))
        };

        // Only the properties a commit sets. A plane has a couple of dozen and
        // naming the ones used is what makes the commit below readable.
        let mut plane_handles = HashMap::new();
        for name in [
            "FB_ID", "CRTC_ID", "SRC_X", "SRC_Y", "SRC_W", "SRC_H", "CRTC_X", "CRTC_Y", "CRTC_W",
            "CRTC_H",
        ] {
            plane_handles.insert(name, need(&plane_props, name)?);
        }

        Ok(Self {
            connector_crtc_id: need(&connector_props, "CRTC_ID")?,
            crtc_mode_id: need(&crtc_props, "MODE_ID")?,
            crtc_active: need(&crtc_props, "ACTIVE")?,
            plane: plane_handles,
            plane_in_fence_fd: plane_props.get("IN_FENCE_FD").copied(),
        })
    }
}

impl ScanoutOutput for KmsOutput {
    fn mode(&self) -> Mode {
        let (width, height) = self.pipeline.mode.size();
        Mode {
            extent: Extent2D::new(width as u32, height as u32),
            // The kernel reports whole hertz; the trait wants millihertz, which
            // is what makes a frame budget worth computing.
            refresh_mhz: self.pipeline.mode.vrefresh() * 1000,
        }
    }

    fn supported_formats(&self) -> &[FormatModifierSet] {
        &self.formats
    }

    fn import_dmabuf(&mut self, buffer: DmaBufPlanes) -> Result<FbHandle> {
        // Each plane's fd becomes a GEM handle. Ownership of the fd transfers
        // here, and the import does not consume it — the fd is closed when the
        // caller's `DmaBufPlane` drops, which is after this returns.
        let mut buffers = Vec::new();
        let mut pitches = [0u32; 4];
        let mut offsets = [0u32; 4];
        let mut handles = [None; 4];
        for (i, plane) in buffer.planes.iter().enumerate().take(4) {
            let handle = self
                .device
                .prime_fd_to_buffer(std::os::fd::AsFd::as_fd(&plane.fd))
                .map_err(|e| err("prime_fd_to_buffer", e))?;
            buffers.push(handle);
            handles[i] = Some(handle);
            pitches[i] = plane.stride;
            offsets[i] = plane.offset;
        }

        let planar = ImportedBuffer {
            extent: buffer.extent,
            fourcc: buffer.fourcc,
            modifier: buffer.modifier,
            pitches,
            offsets,
            handles,
        };
        let fb = self
            .device
            .add_planar_framebuffer(&planar, control::FbCmd2Flags::MODIFIERS)
            .map_err(|e| err("add_planar_framebuffer", e))?;

        let key = u64::from(u32::from(fb));
        self.framebuffers.insert(key, Framebuffer { fb, buffers });
        Ok(FbHandle(key))
    }

    fn release_framebuffer(&mut self, fb: FbHandle) {
        let Some(entry) = self.framebuffers.remove(&fb.0) else {
            return;
        };
        let _ = self.device.destroy_framebuffer(entry.fb);
        // The framebuffer held a reference to each buffer object; closing them
        // afterwards is what actually frees the import.
        for buffer in entry.buffers {
            let _ = self.device.close_buffer(buffer);
        }
    }

    fn commit(&mut self, request: CommitRequest) -> Result<()> {
        let Some(entry) = self.framebuffers.get(&request.fb.0) else {
            return Err(Error::Unsupported(
                "committing a framebuffer this output did not import",
            ));
        };
        let fb = entry.fb;

        let mut atomic = control::atomic::AtomicModeReq::new();
        if request.allow_modeset {
            // Only on the first commit and after a reconfigure: a modeset is
            // far more expensive than a flip, and asking for one every frame
            // is how a frame loop ends up at a fraction of the refresh rate.
            atomic.add_property(
                self.pipeline.connector,
                self.properties.connector_crtc_id,
                control::property::Value::CRTC(Some(self.pipeline.crtc)),
            );
            atomic.add_property(
                self.pipeline.crtc,
                self.properties.crtc_mode_id,
                self.mode_blob,
            );
            atomic.add_property(
                self.pipeline.crtc,
                self.properties.crtc_active,
                control::property::Value::Boolean(true),
            );
        }

        let (width, height) = self.pipeline.mode.size();
        let plane = self.pipeline.plane;
        let p = &self.properties.plane;
        atomic.add_property(
            plane,
            p["FB_ID"],
            control::property::Value::Framebuffer(Some(fb)),
        );
        atomic.add_property(
            plane,
            p["CRTC_ID"],
            control::property::Value::CRTC(Some(self.pipeline.crtc)),
        );
        // Source rectangle is in 16.16 fixed point; the destination is in
        // whole pixels. Mixing the two up scales the frame by 65536.
        atomic.add_property(
            plane,
            p["SRC_X"],
            control::property::Value::UnsignedRange(0),
        );
        atomic.add_property(
            plane,
            p["SRC_Y"],
            control::property::Value::UnsignedRange(0),
        );
        atomic.add_property(
            plane,
            p["SRC_W"],
            control::property::Value::UnsignedRange((width as u64) << 16),
        );
        atomic.add_property(
            plane,
            p["SRC_H"],
            control::property::Value::UnsignedRange((height as u64) << 16),
        );
        atomic.add_property(plane, p["CRTC_X"], control::property::Value::SignedRange(0));
        atomic.add_property(plane, p["CRTC_Y"], control::property::Value::SignedRange(0));
        atomic.add_property(
            plane,
            p["CRTC_W"],
            control::property::Value::UnsignedRange(width as u64),
        );
        atomic.add_property(
            plane,
            p["CRTC_H"],
            control::property::Value::UnsignedRange(height as u64),
        );

        // The render-done fence rides the commit, which is the point of the
        // whole path: the kernel latches the flip when rendering completes
        // rather than the caller blocking until it has.
        //
        // Demonstrated on two devices, and this paragraph used to say the
        // opposite. `the_scanout_target_drives_a_real_display_controller` drives
        // eight frames and asserts `cpu_waits() == 1`: one commit went out
        // without a fence and seven carried one, and every flip completed. It
        // passes against the virtual KMS driver, and it passes on a Raspberry Pi
        // 5 -- `vc4` with HDMI connected, no display server to hold master --
        // where the whole `emblema-present-drm` suite ran on 2026-09-22.
        //
        // What it said before was that the virtual driver never completes a flip
        // for a commit carrying a fence, so the path was believed correct and
        // demonstrated by nothing. The first half of that generalized a narrower
        // limitation: what the virtual driver will not complete is a commit that
        // *also sets a mode*, which `DrmScanoutTarget::present` states precisely
        // and handles by withholding the fence there -- and that withheld fence
        // is the one `cpu_waits` the assertion above allows for. The second half
        // followed from the first and was wrong with it.
        //
        // The fd stays alive until after the commit; the kernel dups what it
        // needs, and closing it earlier hands the kernel a closed descriptor.
        let fence_fd = request.in_fence_fd;
        match (&fence_fd, self.properties.plane_in_fence_fd) {
            (Some(fd), Some(property)) => {
                use std::os::fd::AsRawFd as _;
                atomic.add_property(
                    plane,
                    property,
                    control::property::Value::SignedRange(fd.as_raw_fd() as i64),
                );
            }
            (Some(_), None) => {
                // The caller exported a fence for a driver that cannot take
                // one. It has not waited, so this would commit early.
                return Err(Error::Unsupported(
                    "this driver has no IN_FENCE_FD; the caller must wait before committing",
                ));
            }
            (None, _) => {}
        }

        // Non-blocking with a page-flip event, which is what makes this a frame
        // loop rather than a sequence of stalls: the call returns immediately
        // and the event says when the flip landed.
        let mut flags =
            control::AtomicCommitFlags::PAGE_FLIP_EVENT | control::AtomicCommitFlags::NONBLOCK;
        if request.allow_modeset {
            flags |= control::AtomicCommitFlags::ALLOW_MODESET;
        }
        self.device
            .atomic_commit(flags, atomic)
            .map_err(|e| err("atomic_commit", e))?;
        // Closed only now that the kernel has taken what it needs.
        drop(fence_fd);

        self.in_flight = Some(request.fb);
        Ok(())
    }

    fn poll_events(&mut self) -> Vec<OutputEvent> {
        self.drain_events();
        std::mem::take(&mut self.pending)
    }

    fn wait_for_event(&mut self, timeout_nanos: u64) -> Result<Vec<OutputEvent>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_nanos(timeout_nanos);
        loop {
            self.drain_events();
            if !self.pending.is_empty() {
                return Ok(std::mem::take(&mut self.pending));
            }
            if std::time::Instant::now() >= deadline {
                // Empty rather than an error: a timeout is a frame that has not
                // landed yet, and a caller pacing on this decides what to do
                // about it.
                return Ok(Vec::new());
            }
            // The device fd is blocking, so `receive_events` would wait
            // indefinitely and ignore the deadline. Polling at a fraction of a
            // frame is what respects it without spinning.
            std::thread::sleep(std::time::Duration::from_micros(500));
        }
    }
}

impl KmsOutput {
    /// Framebuffers this output currently holds.
    ///
    /// A frame loop imports one per ring slot and releases it when the slot goes,
    /// so over a run of frames this is constant. It is exposed for the test that
    /// says so: each one holds GEM handles, and a count that climbs is the leak
    /// that would otherwise take a long soak and a careful eye to notice.
    pub fn framebuffer_count(&self) -> usize {
        self.framebuffers.len()
    }

    /// Which vertical blanks this output's flips landed on.
    ///
    /// An inherent method rather than a field on [`OutputEvent::FlipComplete`],
    /// deliberately: that enum is published, and `FakeOutput` and the scanout tests
    /// both construct and match it, so growing a variant would break every caller
    /// for a number only a report wants. If the target or a second binding ever
    /// needs pacing, the event grows a payload then and that is a recorded break.
    pub fn pacing(&self) -> &crate::pacing::Pacing {
        &self.pacing
    }

    /// Nanoseconds between vertical blanks, from the mode's timings rather than
    /// from its rounded refresh.
    ///
    /// [`crate::Mode::frame_nanos`] divides a refresh reported in whole hertz, which
    /// is about a per cent out on the 59.94 Hz modes HDMI is full of. That is
    /// harmless for a wait budget, which is what it is for, and not good enough to
    /// check a blank count against. This computes the period the mode actually
    /// describes: the pixel clock over the total pixels a frame scans.
    ///
    /// `None` where the mode reports no clock or degenerate totals, and where it is
    /// interlaced or doublescan -- both of which make "a frame" mean something this
    /// arithmetic does not handle, and neither of which any display here uses.
    pub fn exact_frame_nanos(&self) -> Option<u64> {
        let mode = &self.pipeline.mode;
        let flags = mode.flags();
        if flags.contains(control::ModeFlags::INTERLACE)
            || flags.contains(control::ModeFlags::DBLSCAN)
        {
            return None;
        }
        let clock = u64::from(mode.clock());
        let htotal = u64::from(mode.hsync().2);
        let vtotal = u64::from(mode.vsync().2);
        if clock == 0 || htotal == 0 || vtotal == 0 {
            return None;
        }
        // The clock is in kilohertz, so the pixels a frame scans divided by it is
        // milliseconds; scaled to nanoseconds without losing the fraction.
        Some(htotal * vtotal * 1_000_000 / clock)
    }

    /// Read whatever the kernel has queued, without blocking.
    fn drain_events(&mut self) {
        // An empty queue reports as an error on a non-blocking fd, which is
        // the ordinary case rather than a fault: most polls find nothing.
        let Ok(events) = self.device.receive_events() else {
            return;
        };
        for event in events {
            if let control::Event::PageFlip(flip) = event {
                // Another controller's blanks are not this one's. A process
                // driving one card with two pipelines gets both here, and
                // counting them together would report a number belonging to
                // neither.
                if flip.crtc != self.pipeline.crtc {
                    continue;
                }
                // `duration` is the kernel's `CLOCK_MONOTONIC` timestamp for the
                // flip, despite the name drm-rs gives it: it is built from
                // `tv_sec` and `tv_usec`, not from a difference. Recorded so the
                // blank count can be cross-checked against the span it arrived
                // over, and never compared with an `Instant`, which shares no
                // epoch with it.
                self.pacing.observe(flip.frame, flip.duration);
                // The flip that completed is the one committed, and what it
                // frees is whatever was on screen before it.
                if let Some(fb) = self.in_flight.take() {
                    self.pending.push(OutputEvent::FlipComplete { fb });
                }
            }
        }
    }
}

impl Drop for KmsOutput {
    fn drop(&mut self) {
        let handles: Vec<u64> = self.framebuffers.keys().copied().collect();
        for key in handles {
            self.release_framebuffer(FbHandle(key));
        }
        // Handed back so another process can drive the card. Without this the
        // lock lives until the fd closes, which is the same moment — but saying
        // it makes the pairing with `become_master` visible.
        let _ = self.device.release_master();
    }
}

/// A buffer already imported, described the way `AddFB2` wants it.
struct ImportedBuffer {
    extent: Extent2D,
    fourcc: emblema_hal::Fourcc,
    modifier: emblema_hal::Modifier,
    pitches: [u32; 4],
    offsets: [u32; 4],
    handles: [Option<drm::buffer::Handle>; 4],
}

impl drm::buffer::PlanarBuffer for ImportedBuffer {
    fn size(&self) -> (u32, u32) {
        (self.extent.width, self.extent.height)
    }

    fn format(&self) -> drm::buffer::DrmFourcc {
        // The fourcc traveled here as the packed code the kernel uses, which
        // is what this enum is over, so an unrecognized one is a format drm-rs
        // has no name for rather than a wrong number.
        drm::buffer::DrmFourcc::try_from(self.fourcc.0).unwrap_or(drm::buffer::DrmFourcc::Xrgb8888)
    }

    fn modifier(&self) -> Option<drm::buffer::DrmModifier> {
        Some(drm::buffer::DrmModifier::from(self.modifier.0))
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

fn err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Backend {
        backend: "drm",
        detail: format!("{what}: {e}"),
    }
}
