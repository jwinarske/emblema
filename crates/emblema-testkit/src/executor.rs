//! Rendering a scene to an image.
//!
//! Generic over the backend, so the same corpus runs against every one. This is
//! the offscreen executor; WSI and DRM executors render the same scenes through
//! their own presentation paths and compare with the same comparators.
//!
//! A scene is turned into a recording by driving [`Canvas`], not by building a
//! batch by hand. That matters more than it sounds. This file used to resolve
//! gradient endpoints, convert clips to scissors, and step the stencil in and
//! out itself — a second implementation of what the canvas does, which drifted
//! from the first as soon as the scene format grew, and which could not express
//! a layer at all because layers are the canvas's own idea. Going through the
//! canvas means the corpus exercises the code that ships, and a scene can say
//! anything the public API can.

use crate::image::Image;
use crate::scene::{Fill, Item, Node, Scene};
use crate::shape::Shape;
use emblema_core::{
    Affine2, Canvas, Color, GradientStop, Layer, Morphology, Paint, Recording, Rect, Shader,
    SourceRect, Sprite, Style, Vec2, Vertices,
};
use emblema_geometry::dash::Dash;
use emblema_hal::{Hal, HalContext, PixelFormat, Result, TextureDescriptor};

fn color_of(c: [f32; 4]) -> Color {
    // Components that came out of a color go back in as they came out.
    Color::srgb(c[0], c[1], c[2], c[3])
}

fn stops_of(stops: &[crate::scene::Stop]) -> Vec<GradientStop> {
    stops
        .iter()
        .map(|stop| GradientStop::new(color_of(stop.color), stop.offset))
        .collect()
}

/// The shader half of a paint.
///
/// Separate from the rest because a node that is not an item has a fill and
/// nothing else to say: no stroke, no dash, no mask blur, no color filter.
/// A mesh is the case that needed it.
fn shader_for(fill: &Fill) -> Shader {
    match fill {
        Fill::Solid(color) => Shader::Solid(color_of(*color)),
        Fill::LinearGradient {
            start,
            end,
            stops,
            tile,
        } => Shader::LinearGradient {
            start: Vec2::from(*start),
            end: Vec2::from(*end),
            stops: stops_of(stops),
            tile: *tile,
        },
        Fill::RadialGradient {
            center,
            radius,
            stops,
            tile,
        } => Shader::RadialGradient {
            center: Vec2::from(*center),
            radius: *radius,
            stops: stops_of(stops),
            tile: *tile,
        },
        Fill::SweepGradient {
            center,
            start_angle,
            end_angle,
            stops,
            tile,
        } => Shader::SweepGradient {
            center: Vec2::from(*center),
            start_angle: *start_angle,
            end_angle: *end_angle,
            stops: stops_of(stops),
            tile: *tile,
        },
        Fill::RuntimeEffect {
            program,
            uniforms,
            images,
        } => Shader::RuntimeEffect {
            // Program zero, always: a scene names no program, and the
            // executor registers exactly one. Registering is idempotent, so
            // this is the same index every time whatever else a caller has
            // registered before it -- provided they registered this one first,
            // which the executor does.
            program: *program,
            uniforms: uniforms.clone(),
            // A scene says which fixture textures its program reads by index,
            // and the executor turns those into the slots it bound them at.
            images: images.clone(),
        },
        Fill::Image {
            rect,
            source,
            tile,
            sampling,
            alpha,
            tint,
        } => Shader::Image {
            // Slot zero, always: a scene names no textures, and the executor
            // supplies exactly one.
            slot: crate::fixture::SLOT,
            rect: Rect::new(rect[0], rect[1], rect[2], rect[3]),
            alpha: *alpha,
            tile: *tile,
            source: Rect::new(source[0], source[1], source[2], source[3]),
            tint: color_of(*tint),
            sampling: *sampling,
        },
        Fill::ConicalGradient {
            start_center,
            start_radius,
            end_center,
            end_radius,
            stops,
            tile,
        } => Shader::ConicalGradient {
            start_center: Vec2::from(*start_center),
            start_radius: *start_radius,
            end_center: Vec2::from(*end_center),
            end_radius: *end_radius,
            stops: stops_of(stops),
            tile: *tile,
        },
    }
}

/// A paint from a fill alone, for the nodes that have no item to speak for
/// them.
fn paint_from(fill: &Fill, anti_alias: bool) -> Paint {
    Paint {
        shader: shader_for(fill),
        anti_alias,
        ..Paint::default()
    }
}

/// How an item is painted.
///
/// `anti_alias` comes from the scene rather than the item because multisampling
/// is a property of the target: a canvas antialiases everything or nothing, and
/// the scene's sample count is the scene saying which.
fn paint_for(item: &Item, anti_alias: bool) -> Paint {
    let shader = shader_for(&item.fill);
    Paint {
        shader,
        // A shape carries no color of its own to combine, so this is the
        // identity. Meshes and sprite batches set it from their own spec.
        tint_blend: emblema_hal::BlendMode::Modulate,
        color_filter: item.color_filter,
        // Not described by the corpus, and for a reason the color filter's
        // presence there makes clearer by contrast. A color filter is
        // arithmetic in the fragment shader, which the two backends reach by
        // separate translations and can disagree about. An image filter is a
        // layer opened and composited, which produces the same recording
        // whatever draws it -- and the passes that recording contains are
        // already compared, by the scenes that blur a layer directly.
        image_filter: item.image_filter.clone(),
        style: match &item.stroke {
            Some(spec) => Style::Stroke(spec.to_style()),
            None => Style::Fill,
        },
        mask_blur: item.mask_blur,
        mask_blur_style: item.mask_blur_style,
        dash: item.stroke.as_ref().and_then(|spec| {
            spec.dash
                .as_ref()
                .map(|(intervals, phase)| Dash::new(intervals.clone(), *phase))
        }),
        blend: item.blend,
        anti_alias,
    }
}

fn rect_of([left, top, right, bottom]: [f32; 4]) -> Rect {
    Rect::new(left, top, right, bottom)
}

/// Record one node and everything under it.
///
/// Each node brackets itself with `save`/`restore`, so nothing it does to the
/// transform, the clip or the stencil reaches its siblings. That is what makes
/// a corpus scene a list of independent things rather than a sequence whose
/// meaning depends on what came before.
fn record_node(canvas: &mut Canvas, node: &Node, anti_alias: bool) -> Result<()> {
    match node {
        Node::Picture(picture) => {
            // Recorded into a canvas of its own and then drawn, which is the
            // whole of what `drawPicture` is. The children go through the same
            // `record_node` they would anywhere else, so a picture may hold a
            // layer, a run, an effect -- and those become passes of the
            // picture's recording rather than of this one, which is the
            // difference the call exists to make.
            let mut inner = Canvas::new(picture.size);
            inner.clear(emblema_core::Color::linear(0.0, 0.0, 0.0, 0.0));
            for child in &picture.children {
                record_node(&mut inner, child, anti_alias)?;
            }
            let recording = inner.finish();
            canvas.save();
            canvas.concat(picture.transform.to_projective());
            let outcome = canvas
                .draw_recording(
                    &recording,
                    &Paint::fill(emblema_core::Color::linear(1.0, 1.0, 1.0, 1.0))
                        .with_blend(picture.blend),
                )
                .err();
            canvas.restore();
            if let Some(e) = outcome {
                return Err(e);
            }
        }
        Node::Paint(paint) => {
            canvas.save();
            canvas.concat(paint.transform.to_projective());
            if let Some(clip) = paint.clip {
                canvas.clip_rect(rect_of(clip))?;
            }
            if let Some(out) = paint.clip_out {
                canvas.clip_out_rect(rect_of(out))?;
            }
            // `draw_color` for a color, rather than `draw_paint` with a solid
            // shader, because that is the call `dart:ui` names for this and the
            // two reach the same place by different routes -- one of which
            // takes the blend as an argument and is therefore the one that can
            // be wrong about it.
            //
            // Anything else is `draw_paint`, which is the call that takes a
            // paint and so the only one a shader can arrive through.
            let result = match &paint.fill {
                Fill::Solid(color) => canvas.draw_color(color_of(*color), paint.blend),
                fill => canvas.draw_paint(&Paint {
                    shader: shader_for(fill),
                    blend: paint.blend,
                    anti_alias,
                    ..Paint::default()
                }),
            }
            .map(|_| ());
            canvas.restore();
            result?;
        }
        Node::NinePatch(nine) => {
            canvas.save();
            canvas.concat(nine.transform.to_projective());
            // The sheet's own size, which the executor knows and the scene
            // must not: a scene names no texture and so cannot name its
            // extent. The center is in texels of that sheet.
            // The paint's shader is replaced per piece by the call, so what
            // it carries here is the alpha, the blend and the antialiasing.
            let mut paint = Paint::image(crate::fixture::SLOT, rect_of(nine.into))
                .with_blend(nine.blend)
                .with_anti_alias(anti_alias);
            // The call replaces the shader for each of the nine pieces, so a
            // scene fading the whole thing sets the alpha on the one it starts
            // from and lets that be carried across.
            if let Shader::Image { alpha, .. } = &mut paint.shader {
                *alpha = nine.alpha;
            }
            let result = canvas
                .draw_image_nine(
                    crate::fixture::SLOT,
                    crate::fixture::SIZE,
                    rect_of(nine.center),
                    rect_of(nine.into),
                    &paint,
                )
                .map(|_| ());
            canvas.restore();
            result?;
        }
        Node::Points(points) => {
            canvas.save();
            canvas.concat(points.transform.to_projective());
            if let Some(shape) = &points.clip_shape {
                canvas.clip_path(&shape.to_path())?;
            }
            let positions: Vec<Vec2> = points.points.iter().copied().map(Vec2::from).collect();
            let mut paint = Paint::stroke(color_of(points.color), points.stroke.width);
            paint.style = Style::Stroke(points.stroke.to_style());
            paint.blend = points.blend;
            paint.anti_alias = anti_alias;
            let result = canvas
                .draw_points(points.mode, &positions, &paint)
                .map(|_| ());
            canvas.restore();
            result?;
        }
        Node::Glyphs(run) => {
            canvas.save();
            canvas.concat(run.transform.to_projective());
            // Built here rather than carried in the scene, for the reason the
            // scene names no texture: an atlas holds device-side coverage and a
            // scene has to be writable without a device. The same indices give
            // the same coverage on both backends, which is what makes a plate
            // over text comparable at all.
            let mut atlas = emblema_text::Atlas::new(64);
            let mut placed = Vec::with_capacity(run.glyphs.len());
            for (index, position) in &run.glyphs {
                let key = emblema_text::GlyphKey {
                    font: 1,
                    glyph: *index as u16,
                    size: 16,
                };
                let coverage = emblema_text::Coverage {
                    width: crate::fixture::GLYPH_SIZE,
                    height: crate::fixture::GLYPH_SIZE,
                    texels: crate::fixture::glyph_coverage(*index),
                };
                let rect = match atlas.insert(key, &coverage) {
                    Ok(rect) => rect,
                    Err(e) => {
                        canvas.restore();
                        return Err(emblema_hal::Error::Backend {
                            backend: "testkit",
                            detail: format!("a fixture glyph did not fit its atlas: {e:?}"),
                        });
                    }
                };
                placed.push(emblema_text::PositionedGlyph::new(key, *position, rect));
            }
            let mut paint = Paint::fill(color_of(run.color))
                .with_blend(run.blend)
                .with_image_filter(run.image_filter.clone());
            if run.mask_blur > 0.0 {
                paint = paint
                    .with_mask_blur(run.mask_blur)
                    .with_mask_blur_style(run.mask_blur_style);
            }
            let outcome = canvas
                .draw_glyphs(&placed, &atlas, crate::fixture::GLYPH_SLOT, &paint)
                .err();
            canvas.restore();
            if let Some(e) = outcome {
                return Err(e);
            }
        }
        Node::Mesh(mesh) => {
            canvas.save();
            canvas.concat(mesh.transform.to_projective());
            let vertices = Vertices::full(
                mesh.mode,
                mesh.positions.iter().copied().map(Vec2::from).collect(),
                mesh.texture_coords
                    .iter()
                    .copied()
                    .map(Vec2::from)
                    .collect(),
                mesh.colors.iter().copied().map(color_of).collect(),
                if mesh.indices.is_empty() {
                    (0..mesh.positions.len() as u32).collect()
                } else {
                    mesh.indices.clone()
                },
            )?;
            let mut paint = paint_from(&mesh.fill, anti_alias);
            paint.blend = mesh.blend;
            paint.tint_blend = mesh.tint_blend;
            paint.image_filter = mesh.image_filter.clone();
            paint.mask_blur = mesh.mask_blur;
            // Restored before the error is raised, or a mesh a device refuses
            // would leave the canvas inside a save nobody closes and every
            // later node in the scene inside it too.
            let result = canvas.draw_vertices(&vertices, &paint).map(|_| ());
            canvas.restore();
            result?;
        }
        Node::Atlas(atlas) => {
            canvas.save();
            let sprites: Vec<Sprite> = atlas
                .sprites
                .iter()
                .map(|s| {
                    Sprite::new(
                        SourceRect::new(
                            s.source[0],
                            s.source[1],
                            s.source[2] - s.source[0],
                            s.source[3] - s.source[1],
                        ),
                        Affine2::from_scale_angle_translation(
                            Vec2::splat(s.scale),
                            s.rotate,
                            Vec2::from(s.translate),
                        ),
                    )
                    .with_color(color_of(s.color))
                })
                .collect();
            // The paint's own rectangle is never mapped through for a sprite
            // batch -- each sprite's source rectangle says what it reads --
            // so this only has to be a well-formed one.
            let paint = Paint::image(crate::fixture::SLOT, Rect::new(0.0, 0.0, 1.0, 1.0))
                .with_image_alpha(atlas.alpha)
                .with_blend(atlas.blend)
                .with_tint_blend(atlas.tint_blend);
            let result = canvas
                .draw_atlas(&sprites, crate::fixture::SIZE, &paint)
                .map(|_| ());
            canvas.restore();
            result?;
        }
        Node::Shadow(shadow) => {
            canvas.save();
            canvas.concat(shadow.transform.to_projective());
            let path = shadow.shape.to_path();
            let mut result = canvas
                .draw_shadow(
                    &path,
                    color_of(shadow.color),
                    shadow.elevation,
                    shadow.transparent_occluder,
                )
                .map(|_| ());
            if result.is_ok() && shadow.with_caster {
                // The object itself, which is what makes the shadow legible:
                // an outer shadow with nothing on top is a picture of a hole.
                result = canvas
                    .draw_path(&path, &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)))
                    .map(|_| ());
            }
            canvas.restore();
            result?;
        }
        Node::Draw(item) => {
            canvas.save();
            canvas.concat(item.transform.to_projective());
            if let Some(clip) = item.clip {
                canvas.clip_rect(rect_of(clip))?;
            }
            if let Some(shape) = &item.clip_shape {
                canvas.clip_path(&shape.to_path())?;
            }
            // After the narrowing clips, which is the order that makes a scene
            // stating both mean what it reads as: keep to this, then not that.
            if let Some(out) = item.clip_out {
                canvas.clip_out_rect(rect_of(out))?;
            }
            let paint = paint_for(item, anti_alias);
            // A rounded rectangle goes through the call the public API offers
            // for it rather than through its path, so the corpus exercises
            // whichever way that call decides to draw it. Sending the path
            // instead would pin the corpus to the tessellated one and leave the
            // choice untested by everything the corpus drives.
            match &item.shape {
                // A scene can ask for the path route explicitly, which is the
                // only way to reach it for a shape that has its own call.
                _ if item.as_path => {
                    canvas.draw_path(&item.shape.to_path(), &paint)?;
                }
                // A circle goes through its own call for the same reason: that
                // is where the choice between a distance field and four cubics
                // is made, and handing over a path would decide it here.
                Shape::Line { from, to } => {
                    canvas.draw_line(Vec2::from(*from), Vec2::from(*to), &paint)?;
                }
                Shape::Rect { min, max } => {
                    canvas.draw_rect(Rect::new(min[0], min[1], max[0], max[1]), &paint)?;
                }
                Shape::Circle { center, radius } => {
                    canvas.draw_circle(Vec2::from(*center), *radius, &paint)?;
                }
                Shape::Oval { min, max } => {
                    canvas.draw_oval(Rect::new(min[0], min[1], max[0], max[1]), &paint)?;
                }
                Shape::RoundedRect { min, max, radius } => {
                    canvas.draw_rrect(
                        Rect::new(min[0], min[1], max[0], max[1]),
                        *radius,
                        &paint,
                    )?;
                }
                Shape::RoundedRectWithRadii { min, max, radii } => {
                    canvas.draw_rrect_with_radii(
                        Rect::new(min[0], min[1], max[0], max[1]),
                        *radii,
                        &paint,
                    )?;
                }
                Shape::DiffRoundedRectWithRadii {
                    outer,
                    outer_radii,
                    inner,
                    inner_radii,
                } => {
                    canvas.draw_drrect_with_radii(
                        Rect::new(outer[0][0], outer[0][1], outer[1][0], outer[1][1]),
                        *outer_radii,
                        Rect::new(inner[0][0], inner[0][1], inner[1][0], inner[1][1]),
                        *inner_radii,
                        &paint,
                    )?;
                }
                Shape::DiffRoundedRect {
                    outer,
                    outer_radius,
                    inner,
                    inner_radius,
                } => {
                    canvas.draw_drrect(
                        Rect::new(outer[0][0], outer[0][1], outer[1][0], outer[1][1]),
                        *outer_radius,
                        Rect::new(inner[0][0], inner[0][1], inner[1][0], inner[1][1]),
                        *inner_radius,
                        &paint,
                    )?;
                }
                shape => {
                    canvas.draw_path(&shape.to_path(), &paint)?;
                }
            }
            canvas.restore();
        }
        Node::Layer {
            layer,
            bounds,
            transform,
            children,
        } => {
            canvas.save();
            canvas.concat(transform.to_projective());
            // Taken before the binding below shadows the spec it came from.
            let backdrop = layer.backdrop.clone();
            let filter = layer.filter.clone();
            let layer = Layer {
                blur: Vec2::splat(layer.blur),
                alpha: layer.alpha,
                blend: layer.blend,
                // The scene format has no way to say a group is transformed on
                // the way back. Its `transform` moves what goes *into* the
                // group, which is a different thing, and conflating the two
                // would make every existing scene resample where it used to
                // redraw.
                matrix: layer.matrix.map(|m| m.to_projective()),
                backdrop_blur: layer.backdrop_blur,
                backdrop_id: layer.backdrop_id,
                color_filter: layer.color_filter,
                morphology: layer.morphology.map(|m| {
                    if m.dilate {
                        Morphology::dilate(m.radius[0], m.radius[1])
                    } else {
                        Morphology::erode(m.radius[0], m.radius[1])
                    }
                }),
            };
            // A general backdrop goes through the call that takes one, and a
            // scene that names none takes the path every other plate takes --
            // so the two are not two spellings of the same thing here, and a
            // plate that asked for a matrix backdrop reports the refusal rather
            // than drawing something else.
            if !filter.is_identity() {
                canvas.save_layer_filtered(layer, bounds.map(rect_of), &filter)?;
            } else if backdrop.is_identity() {
                match bounds {
                    Some(bounds) => canvas.save_layer_bounds(layer, rect_of(*bounds)),
                    None => canvas.save_layer(layer),
                };
            } else {
                canvas.save_layer_backdrop(layer, bounds.map(rect_of), &backdrop)?;
            }
            for child in children {
                record_node(canvas, child, anti_alias)?;
            }
            canvas.restore();
            canvas.restore();
        }
        Node::Clip {
            rect,
            rect_out,
            shape,
            transform,
            children,
        } => {
            // A save and a restore around the clip, which is what makes it a
            // clip in force over a run rather than one narrowing a draw: the
            // canvas carries it until the restore, and everything recorded
            // between them is under it.
            canvas.save();
            // Before the clip is taken, so the shape and the rectangles are
            // stated in the space this opens rather than in the frame's, and
            // what the clip scopes is drawn under it too.
            canvas.concat(transform.to_projective());
            if let Some(rect) = rect {
                canvas.clip_rect(rect_of(*rect))?;
            }
            if let Some(out) = rect_out {
                canvas.clip_out_rect(rect_of(*out))?;
            }
            if let Some(shape) = shape {
                canvas.clip_path(&shape.to_path())?;
            }
            for child in children {
                record_node(canvas, child, anti_alias)?;
            }
            canvas.restore();
        }
    }
    Ok(())
}

/// Turn a scene into a recording.
///
/// Shared rather than duplicated per caller: a second copy of this drifted out
/// of step the moment the scene format grew, which is how a frame-loop test
/// ended up reading a field that no longer existed.
pub fn record_scene(scene: &Scene) -> Result<Recording> {
    record_scene_with(scene, Culling::Applied)
}

/// Whether the recording keeps the draws occlusion culling would remove.
///
/// `Kept` is what makes culling testable at all: the same scene recorded both
/// ways must render to the same bytes, since culling is defined as removing
/// only what nothing can see. A predicate that admits a draw it should not makes
/// the two differ, which is how a translucent mesh treated as an occluder was
/// found -- see `occludes` in `emblema-hal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Culling {
    Applied,
    Kept,
}

/// Turn a scene into a recording, saying whether culling runs.
pub fn record_scene_with(scene: &Scene, culling: Culling) -> Result<Recording> {
    let mut canvas = Canvas::new(scene.size).with_samples(scene.samples);
    if culling == Culling::Kept {
        canvas.keep_hidden_draws();
    }
    canvas.clear(color_of(scene.background));
    let anti_alias = scene.samples > 1;
    for node in &scene.items {
        record_node(&mut canvas, node, anti_alias)?;
    }
    Ok(canvas.finish())
}

/// Render a scene offscreen and read it back.
pub fn render_scene<H: Hal>(ctx: &mut H::Context, scene: &Scene) -> Result<Image>
where
    H::Context: HalContext<Hal = H>,
{
    render_scene_into::<H>(ctx, scene, PixelFormat::Rgba8Unorm)
}

/// The same, into a target of the given format.
///
/// Every comparison in this crate renders into linear eight-bit color, which
/// leaves the path where the attachment encodes on write almost unexercised --
/// and the two backends reach that path by different means, one through an
/// image view and the other through a framebuffer whose attachment carries the
/// format. Two implementations of the same conversion is exactly what a
/// comparison between them is for.
///
/// The format has to stay four bytes per pixel: [`Image`] is a comparison
/// surface rather than a general one, and nothing here compares two wide
/// images.
pub fn render_scene_into<H: Hal>(
    ctx: &mut H::Context,
    scene: &Scene,
    format: PixelFormat,
) -> Result<Image>
where
    H::Context: HalContext<Hal = H>,
{
    render_scene_culled_into::<H>(ctx, scene, format, Culling::Applied)
}

/// The same, saying whether culling runs on the recording first.
pub fn render_scene_culled_into<H: Hal>(
    ctx: &mut H::Context,
    scene: &Scene,
    format: PixelFormat,
    culling: Culling,
) -> Result<Image>
where
    H::Context: HalContext<Hal = H>,
{
    let recording = record_scene_with(scene, culling)?;
    // Uploaded per scene that asks for it rather than held by the caller,
    // which keeps every consumer of this function -- the window, the
    // comparison, the sheet renderer -- from having to know that some scenes
    // sample a texture. A scene that does not ask allocates nothing.
    // Registered before anything is drawn and before the sheet, so the index
    // the scenes name is the one it gets. Idempotent, so a list of scenes
    // costs one program rather than one per scene.
    // All of them, always, and in this order. A scene names a program by the
    // index it was given, so registering only the ones a scene appears to want
    // would make that index depend on which scene ran first.
    //
    // Unconditional rather than gated on whether the scene uses one, which it
    // was until a derivation deciding that got it wrong for the fourth time. A
    // group whose *backdrop* is a program has nothing inside it naming one, so
    // the gate said no and the programs went unregistered -- and the catalog
    // passed anyway, another scene having registered them first, which made the
    // failure an ordering one that a catalog cannot show and a single scene
    // always does.
    //
    // The gate was never worth it. Registering is storing the SPIR-V payload
    // against an index; pipelines are built lazily and keyed by it, so what a
    // scene that uses no program pays is three clones of a byte vector, once
    // per context. That is cheaper than a derivation which has to be right.
    for (expected, program) in [
        (0, crate::fixture::effect()),
        (1, crate::fixture::two_image_effect()),
        (2, crate::fixture::image_effect()),
        (3, crate::fixture::mesh_uv_effect()),
    ] {
        let id = ctx.register_program(&program)?;
        if id != expected {
            return Err(emblema_hal::Error::Unsupported(
                "a fixture program was not registered at the index scenes name it by",
            ));
        }
    }
    let fixtures = Fixtures::<H>::prepare(ctx, scene)?;
    let result =
        emblema_core::render_offscreen_into::<H>(ctx, &recording, &fixtures.bound(), format);
    // Released whether the draw worked or not: a scene that fails to render
    // must not leak a texture into every later scene's device.
    fixtures.destroy(ctx);
    let pixels = result?;
    Ok(Image::new(scene.size.width, scene.size.height, pixels))
}

/// The textures a scene named, uploaded and ready to bind.
///
/// Extracted so that anything rendering a scene binds the same textures in the
/// same slots. Recording already goes through this crate for that reason -- a
/// second copy of it stopped matching the first as soon as the scene format
/// grew -- and the texture table is the same kind of thing: the frame-loop test
/// passed an empty one, which was correct until the corpus first held a scene
/// that reads a texture, and then was a failure about slots rather than about
/// presenting.
pub struct Fixtures<H: Hal> {
    sheet: Option<H::Texture>,
    glyphs: Option<H::Texture>,
}

impl<H: Hal> Fixtures<H> {
    /// Upload whatever `scene` says it reads.
    pub fn prepare(ctx: &mut H::Context, scene: &Scene) -> Result<Self>
    where
        H::Context: HalContext<Hal = H>,
    {
        let sheet = if scene.samples_fixture() {
            // With a chain, so a plate can draw the sheet smaller than its own
            // size and mean it. Costs four levels over an eight-by-eight sheet
            // and changes nothing for the plates that do not ask: only
            // mipmapped sampling reads past the first level, and every other
            // quality names level zero outright.
            let mut sheet = ctx.create_texture(&TextureDescriptor::mipmapped(
                crate::fixture::SIZE,
                PixelFormat::Rgba8Unorm,
            ))?;
            if let Err(e) = ctx.write_texture(&mut sheet, &crate::fixture::pixels()) {
                ctx.destroy_texture(sheet);
                return Err(e);
            }
            Some(sheet)
        } else {
            None
        };

        let glyphs = if scene.uses_glyphs() {
            let side = 64;
            let made = ctx
                .create_texture(&TextureDescriptor::offscreen(
                    emblema_hal::Extent2D::new(side, side),
                    PixelFormat::Rgba8Unorm,
                ))
                .and_then(|mut texture| {
                    let mut atlas = emblema_text::Atlas::new(side);
                    for index in 0..4u32 {
                        let key = emblema_text::GlyphKey {
                            font: 1,
                            glyph: index as u16,
                            size: 16,
                        };
                        let _ = atlas.insert(
                            key,
                            &emblema_text::Coverage {
                                width: crate::fixture::GLYPH_SIZE,
                                height: crate::fixture::GLYPH_SIZE,
                                texels: crate::fixture::glyph_coverage(index),
                            },
                        );
                    }
                    // The atlas holds one byte per texel and the texture four,
                    // so the coverage is spread across all of them. Any channel
                    // would serve, since the material reads red; writing all
                    // four is what makes the texture legible to anyone looking.
                    let mut texels = vec![0u8; (side * side * 4) as usize];
                    for (i, coverage) in atlas.texels().iter().enumerate() {
                        texels[i * 4..i * 4 + 4].copy_from_slice(&[*coverage; 4]);
                    }
                    ctx.write_texture(&mut texture, &texels).map(|()| texture)
                });
            match made {
                Ok(texture) => Some(texture),
                Err(e) => {
                    if let Some(sheet) = sheet {
                        ctx.destroy_texture(sheet);
                    }
                    return Err(e);
                }
            }
        } else {
            None
        };
        Ok(Self { sheet, glyphs })
    }

    /// The textures in the slots a scene names them by.
    ///
    /// The sheet is slot zero and the glyph atlas slot one, whichever of them
    /// the scene actually reads -- so the number a scene states does not depend
    /// on what else it draws. Where the sheet is absent and the atlas is not,
    /// the atlas stands in at zero as well: nothing samples that slot, and a
    /// table with a hole in it is not a table.
    pub fn bound(&self) -> Vec<&H::Texture> {
        match (self.sheet.as_ref(), self.glyphs.as_ref()) {
            (Some(sheet), Some(glyphs)) => vec![sheet, glyphs],
            (Some(sheet), None) => vec![sheet],
            (None, Some(glyphs)) => vec![glyphs, glyphs],
            (None, None) => Vec::new(),
        }
    }

    pub fn destroy(self, ctx: &mut H::Context)
    where
        H::Context: HalContext<Hal = H>,
    {
        if let Some(sheet) = self.sheet {
            ctx.destroy_texture(sheet);
        }
        if let Some(glyphs) = self.glyphs {
            ctx.destroy_texture(glyphs);
        }
    }
}

/// Render every scene in a corpus.
///
/// A scene that fails is reported with its name rather than aborting the run,
/// so one broken capability does not hide the state of everything else.
pub fn render_corpus<H: Hal>(
    ctx: &mut H::Context,
    scenes: &[Scene],
) -> Vec<(&'static str, Result<Image>)>
where
    H::Context: HalContext<Hal = H>,
{
    scenes
        .iter()
        .map(|scene| (scene.name, render_scene::<H>(ctx, scene)))
        .collect()
}
