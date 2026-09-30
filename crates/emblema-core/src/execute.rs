//! Rendering a recording, pass by pass.
//!
//! Here rather than in the crate that wraps a backend, because a recording is
//! defined here and executing one is nothing more than reading it: a layer is a
//! target allocated at that pass's size, rendered into, and sampled by a later
//! pass. Nothing about that is backend-specific, so it is written once and
//! generic over the HAL. It had been private to the public crate, which left
//! the test harness with the choice of duplicating it or staying at the batch
//! level where layers cannot be expressed.

use crate::canvas::{Pass, Recording, TextureSource};
use crate::ramp::RAMP_WIDTH;
use emblema_hal::{
    Error, Extent2D, Hal, HalContext, HalTexture, PixelFormat, Result, TextureDescriptor,
};

/// The textures a recording needs that it did not arrive with.
///
/// Layer targets are rendered and baked gradients are uploaded, both per
/// submission and both released with it. Grouped because they are the same
/// thing from the caller's side -- resources this frame owns and must destroy
/// after the work that reads them -- and because a caller that has to submit
/// the root pass itself would otherwise be handed two lists and trusted to
/// keep them together.
pub struct Transient<H: Hal> {
    /// One per non-root pass, in pass order.
    pub layers: Vec<H::Texture>,
    /// One per entry in [`Recording::ramps`], in that order.
    pub ramps: Vec<H::Texture>,
}

impl<H: Hal> Transient<H> {
    fn empty() -> Self {
        Self {
            layers: Vec::new(),
            ramps: Vec::new(),
        }
    }

    /// Everything this frame owns, as one list to release when it is done.
    ///
    /// For a caller that keeps the resources alive past this call -- a
    /// presentation target holding them until its fence signals -- where a
    /// texture came from stops mattering the moment the submission is made.
    /// What remains is when it is safe to free, and that answer is the same for
    /// all of them.
    pub fn into_textures(self) -> Vec<H::Texture> {
        let mut all = self.layers;
        all.extend(self.ramps);
        all
    }

    /// Release everything, in any order: nothing here refers to anything else.
    pub fn destroy(self, ctx: &mut H::Context)
    where
        H::Context: HalContext<Hal = H>,
    {
        for texture in self.layers.into_iter().chain(self.ramps) {
            ctx.destroy_texture(texture);
        }
    }
}

/// Upload every baked gradient the recording carries.
///
/// The format is linear half-float, and the argument the sRGB one rested on is
/// answered rather than overridden. That argument was about how to spend eight
/// bits: eight bits of *linear* color band visibly in the darks, so spacing
/// them through the transfer function put them where the eye reads them. Half
/// has no fixed quantum -- its precision is relative, about eleven bits of
/// mantissa at every magnitude -- so there are no longer eight bits to spend
/// well, and the perceptual spacing was buying what the format now gives
/// everywhere.
///
/// What it also gives is range. A gradient stop outside the sRGB primaries has
/// a component outside zero to one, and eight bits through a transfer function
/// had nowhere to put one.
///
/// Sampled and never drawn into, which is worth saying rather than leaving to
/// a backend to assume: a ramp asked for a color attachment it had no use for,
/// and a backend that builds one per texture would refuse a format it could
/// perfectly well have sampled.
fn upload_ramps<H: Hal>(ctx: &mut H::Context, recording: &Recording) -> Result<Vec<H::Texture>>
where
    H::Context: HalContext<Hal = H>,
{
    let mut uploaded = Vec::with_capacity(recording.ramps.len());
    for ramp in &recording.ramps {
        let extent = Extent2D::new(RAMP_WIDTH as u32, 1);
        let outcome = ctx
            .create_texture(&TextureDescriptor::sampled(
                extent,
                PixelFormat::Rgba16Float,
            ))
            .and_then(
                |mut texture| match ctx.write_texture(&mut texture, &ramp.texels) {
                    Ok(()) => Ok(texture),
                    Err(e) => {
                        ctx.destroy_texture(texture);
                        Err(e)
                    }
                },
            );
        match outcome {
            Ok(texture) => uploaded.push(texture),
            Err(e) => {
                for texture in uploaded {
                    ctx.destroy_texture(texture);
                }
                return Err(e);
            }
        }
    }
    Ok(uploaded)
}

/// Resolve one pass's slot table against the caller's images and the layers
/// rendered so far.
///
/// A slot means something different in each pass, since a layer occupies one
/// alongside the caller's images and the two number independently. Resolving
/// per pass is what lets that work.
pub fn resolve_sources<'a, H: Hal>(
    pass: &Pass,
    images: &[&'a H::Texture],
    transient: &'a Transient<H>,
) -> Result<Vec<&'a H::Texture>> {
    let mut table = Vec::with_capacity(pass.sources.len());
    for source in &pass.sources {
        match source {
            TextureSource::Image(slot) => {
                table.push(*images.get(*slot as usize).ok_or(Error::Unsupported(
                    "a paint samples an image the caller did not supply",
                ))?);
            }
            // Earlier by construction, so this is always already rendered. A
            // recording that named a later one would be malformed rather than
            // merely out of order.
            TextureSource::Layer(pass_index) => {
                table.push(transient.layers.get(*pass_index).ok_or(Error::Unsupported(
                    "a layer is composited before it is rendered",
                ))?);
            }
            // Uploaded before any pass runs, so unlike a layer this cannot be
            // named too early. A miss means the recording and its ramp table
            // disagree, which is malformed rather than merely out of order.
            TextureSource::Ramp(index) => {
                table.push(transient.ramps.get(*index).ok_or(Error::Unsupported(
                    "a gradient names a ramp the recording does not carry",
                ))?);
            }
        }
    }
    Ok(table)
}

/// Render every pass except the root, returning the layer targets in order.
///
/// Split out from [`execute`] so that a caller who has to submit the root pass
/// itself can still get the layers rendered. That is not a hypothetical: the
/// root is the pass that lands in a presentable image, and presenting one means
/// submitting it gated on the acquire and present semaphores — which is a
/// different call from the one that renders offscreen and waits. Without this
/// split, a frame with layers could be rendered offscreen and never presented.
///
/// The returned textures are the caller's to destroy, and must outlive the
/// submission that samples them.
pub fn execute_layers<H: Hal>(
    ctx: &mut H::Context,
    recording: &Recording,
    images: &[&H::Texture],
    format: PixelFormat,
) -> Result<Transient<H>>
where
    H::Context: HalContext<Hal = H>,
{
    // Gradients first: a layer pass can sample one, so they have to exist
    // before any pass runs rather than before the root pass.
    let mut transient = Transient::<H>::empty();
    transient.ramps = upload_ramps::<H>(ctx, recording)?;

    // Layers rendered in order, so one is always finished before the pass that
    // samples it -- which is the order a recording stores them in, since a
    // layer is filed when it is restored and cannot be composited before that.
    for pass in &recording.passes[..recording.passes.len() - 1] {
        let outcome = resolve_sources::<H>(pass, images, &transient).and_then(|table| {
            let mut target =
                ctx.create_texture(&TextureDescriptor::offscreen(pass.extent, format))?;
            let result =
                ctx.submit_batch_textured(&mut target, &pass.batch, pass.descriptor, &table);
            Ok((target, result))
        });
        match outcome {
            Ok((target, result)) => {
                // The target joins the list whether or not the draw succeeded,
                // so a failure releases it with the rest rather than leaking.
                transient.layers.push(target);
                if let Err(e) = result {
                    transient.destroy(ctx);
                    return Err(e);
                }
            }
            Err(e) => {
                transient.destroy(ctx);
                return Err(e);
            }
        }
    }
    Ok(transient)
}

/// Render every pass of a recording, layers first, root into the surface.
///
/// `images` are the caller's own textures, addressed by the slot a paint named.
///
/// Layer targets are allocated per call and released before returning. Pooling
/// them was assumed to be worth doing and is not: creating and destroying a
/// 512x512 target measures 0.6 microseconds against 53 for the submission that
/// draws into it, so a pool would save under two percent of what a pass costs
/// and would owe an invalidation rule in exchange.
pub fn execute<H: Hal>(
    ctx: &mut H::Context,
    surface: &mut H::Texture,
    recording: &Recording,
    images: &[&H::Texture],
) -> Result<()>
where
    H::Context: HalContext<Hal = H>,
{
    let transient = execute_layers::<H>(ctx, recording, images, surface.format().intermediate())?;
    let root = recording.root();
    let outcome = resolve_sources::<H>(root, images, &transient)
        .and_then(|table| ctx.submit_batch_textured(surface, &root.batch, root.descriptor, &table));
    transient.destroy(ctx);
    outcome
}

/// Render a recording into a texture this function allocates, and read it back.
///
/// The offscreen path, which is what a comparison run wants: no surface, no
/// presentation, just the pixels. The target is released whichever way the
/// render went, so a failing recording does not leak a texture into the rest of
/// a run.
pub fn render_offscreen<H: Hal>(
    ctx: &mut H::Context,
    recording: &Recording,
    images: &[&H::Texture],
) -> Result<Vec<u8>>
where
    H::Context: HalContext<Hal = H>,
{
    render_offscreen_into::<H>(ctx, recording, images, PixelFormat::Rgba8Unorm)
}

/// The same, into a target of the caller's choosing.
///
/// Separate rather than a parameter on the call above, because every caller in
/// this workspace wants eight bits and threading the answer through all of them
/// to serve the one that does not is the wrong way round. What this exists for
/// is a target that can hold a color the sRGB primaries cannot describe, which
/// needs a floating-point format and nothing else here does.
///
/// The bytes come back in whatever the format packs them as -- half-floats for
/// `Rgba16Float` -- rather than converted to anything.
pub fn render_offscreen_into<H: Hal>(
    ctx: &mut H::Context,
    recording: &Recording,
    images: &[&H::Texture],
    format: PixelFormat,
) -> Result<Vec<u8>>
where
    H::Context: HalContext<Hal = H>,
{
    let mut target = ctx.create_texture(&TextureDescriptor::offscreen(recording.extent, format))?;
    let outcome = execute::<H>(ctx, &mut target, recording, images)
        .and_then(|()| ctx.read_texture(&mut target));
    ctx.destroy_texture(target);
    outcome
}

/// Render a recording's layers, then submit its root without waiting.
///
/// The shape a display path needs. Everything but the root is a prerequisite of
/// it rather than part of the frame's pacing, so those go through the waiting
/// submission and are ordered before the root by having finished; only the root
/// produces a fence, because the fence is what a page flip is gated on and the
/// root is the pass that fills the image being flipped.
///
/// Both halves of the return travel together until the fence retires: the layer
/// targets are what the submission is still sampling. Handing them back rather
/// than hiding them is deliberate — a caller that owns the fence is the only
/// one that knows when it retires.
pub fn execute_deferred<H: Hal>(
    ctx: &mut H::Context,
    surface: &mut H::Texture,
    recording: &Recording,
    images: &[&H::Texture],
) -> Result<(H::Fence, Transient<H>)>
where
    H::Context: HalContext<Hal = H>,
{
    let transient = execute_layers::<H>(ctx, recording, images, surface.format().intermediate())?;
    let root = recording.root();
    let outcome = resolve_sources::<H>(root, images, &transient).and_then(|table| {
        ctx.submit_batch_deferred_textured(surface, &root.batch, root.descriptor, &table)
    });
    match outcome {
        Ok(fence) => Ok((fence, transient)),
        Err(e) => {
            // No fence means nothing is reading them, so this is the moment.
            transient.destroy(ctx);
            Err(e)
        }
    }
}
