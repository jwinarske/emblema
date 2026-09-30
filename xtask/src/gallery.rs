//! Every corpus scene on one sheet, for a person to look at.
//!
//! The corpus is compared numerically and thoroughly: backend against backend,
//! device against device, each scene against a mutation of itself. None of that
//! can see a scene that is wrong in a way both implementations agree on. A
//! gradient banded the same way everywhere, a shape drawn in the wrong place, a
//! color that is arithmetically correct and far too dark -- every comparison
//! here passes on all of them.
//!
//! That last one is not hypothetical. The bundled example rendered into a
//! linear surface and wrote the bytes to a file for a long time, which every
//! viewer reads as sRGB; the picture was uniformly far too dark and nothing in
//! the suite could have said so. It was found by somebody looking at it.
//!
//! So this makes looking cheap. One command, one sheet, and the grid printed
//! alongside so a tile can be found by counting.
//!
//! What it is good for is gross wrongness: a blank tile, a shape in the wrong
//! place, a color inverted, a scene that is far too dark. It is not good for
//! fine judgment, and the first two things that looked wrong on it were not.
//! One tile appeared to have a gray surround and did not -- black corners, and
//! forty distinct colors along an antialiased edge. Another appeared to blur
//! one of its two shapes and not the other; both were blurred, and the larger
//! one simply shows less of it. Measure before believing either.
//!
//! Written as a PPM for the same reason the example is: every viewer reads one
//! and it needs no encoder, where taking an image dependency to write a nicer
//! file would cost this workspace its pure-Rust build for a convenience.

use emblema_hal_vulkan::{DevicePreference, VulkanContext, VulkanHal};
use emblema_testkit::{corpus, render_scene, Image};

/// Gap between tiles, in pixels, and the color behind them.
const GAP: u32 = 8;
const BACKDROP: [u8; 3] = [32, 32, 36];
/// Marks a scene this device could not render, so a hole in the sheet is
/// distinguishable from a scene that drew nothing.
const UNSUPPORTED: [u8; 3] = [90, 40, 40];

pub struct Sheet {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub drawn: usize,
    pub skipped: Vec<&'static str>,
    /// Scene names in the order they were laid out, row by row.
    ///
    /// The sheet carries no labels: drawing text would need a font, which this
    /// workspace deliberately does not have. Printing the grid alongside it
    /// costs nothing and answers the only question a label would -- which tile
    /// is the one that looks wrong.
    pub names: Vec<&'static str>,
    pub columns: u32,
    /// Scenes the preferred device could not render, drawn by a fallback.
    ///
    /// Reported because the sheet does not say so: whether two devices agree is
    /// a separate question the conformance run answers, and a tile drawn by the
    /// software rasterizer should not be read as evidence about the hardware.
    pub borrowed: Vec<&'static str>,
}

/// Render the corpus and lay it out in a grid.
pub fn render(columns: u32) -> Result<Sheet, String> {
    // Every device this machine offers, preferred first. A scene the preferred
    // one cannot render is drawn by whichever can, because the alternative is a
    // sheet with holes in exactly the places least looked at: the advanced
    // blend modes need an extension this hardware lacks and the software
    // rasterizer has, so without this they are the six scenes nobody ever sees.
    let mut devices: Vec<VulkanContext> = [DevicePreference::Auto, DevicePreference::Software]
        .into_iter()
        .filter_map(|preference| VulkanContext::new(preference).ok())
        .collect();
    if devices.is_empty() {
        return Err("no Vulkan device".to_string());
    }

    let scenes = corpus();
    let mut tiles: Vec<(&'static str, Option<Image>)> = Vec::with_capacity(scenes.len());
    let mut borrowed = Vec::new();
    for scene in &scenes {
        let chosen = devices
            .iter()
            .position(|ctx| scene.supported_by(ctx.capabilities()));
        let Some(index) = chosen else {
            tiles.push((scene.name, None));
            continue;
        };
        if index > 0 {
            borrowed.push(scene.name);
        }
        match render_scene::<VulkanHal>(&mut devices[index], scene) {
            Ok(image) => tiles.push((scene.name, Some(image))),
            Err(e) => {
                eprintln!("{}: {e}", scene.name);
                tiles.push((scene.name, None));
            }
        }
    }

    // Every corpus scene is the same size today. Taking the largest rather than
    // assuming that keeps the sheet correct if one stops being.
    let cell_width = tiles
        .iter()
        .filter_map(|(_, image)| image.as_ref().map(|i| i.width))
        .max()
        .unwrap_or(128);
    let cell_height = tiles
        .iter()
        .filter_map(|(_, image)| image.as_ref().map(|i| i.height))
        .max()
        .unwrap_or(128);

    let columns = columns.max(1);
    let rows = tiles.len().div_ceil(columns as usize) as u32;
    let width = columns * cell_width + (columns + 1) * GAP;
    let height = rows * cell_height + (rows + 1) * GAP;

    let mut pixels = Vec::with_capacity((width * height * 3) as usize);
    for _ in 0..width * height {
        pixels.extend_from_slice(&BACKDROP);
    }

    let mut skipped = Vec::new();
    for (index, (name, image)) in tiles.iter().enumerate() {
        let column = index as u32 % columns;
        let row = index as u32 / columns;
        let left = GAP + column * (cell_width + GAP);
        let top = GAP + row * (cell_height + GAP);
        let Some(image) = image else {
            skipped.push(*name);
            fill(
                &mut pixels,
                width,
                left,
                top,
                cell_width,
                cell_height,
                UNSUPPORTED,
            );
            continue;
        };
        blit(&mut pixels, width, left, top, image);
    }

    Ok(Sheet {
        width,
        height,
        pixels,
        drawn: tiles.len() - skipped.len(),
        skipped,
        names: tiles.iter().map(|(name, _)| *name).collect(),
        columns,
        borrowed,
    })
}

fn fill(sheet: &mut [u8], stride: u32, left: u32, top: u32, w: u32, h: u32, color: [u8; 3]) {
    for y in 0..h {
        for x in 0..w {
            let at = (((top + y) * stride + left + x) * 3) as usize;
            sheet[at..at + 3].copy_from_slice(&color);
        }
    }
}

/// Composite a rendered scene over the backdrop.
///
/// Over rather than into: a scene clears to transparent unless it says
/// otherwise, and dropping its alpha would show the difference between a
/// transparent region and a black one as nothing at all -- which is exactly the
/// kind of thing a sheet exists to make visible.
fn blit(sheet: &mut [u8], stride: u32, left: u32, top: u32, image: &Image) {
    for y in 0..image.height {
        for x in 0..image.width {
            let from = ((y * image.width + x) * 4) as usize;
            let to = (((top + y) * stride + left + x) * 3) as usize;
            let alpha = image.pixels[from + 3] as u32;
            for channel in 0..3 {
                // The scene's color is premultiplied, so the backdrop is
                // scaled by what is left rather than the two being mixed.
                let over = image.pixels[from + channel] as u32;
                let under = sheet[to + channel] as u32;
                sheet[to + channel] = (over + under * (255 - alpha) / 255).min(255) as u8;
            }
        }
    }
}

pub fn write_ppm(path: &str, sheet: &Sheet) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    write!(out, "P6\n{} {}\n255\n", sheet.width, sheet.height)?;
    out.write_all(&sheet.pixels)?;
    out.flush()
}

/// The grid, as text, so a tile can be found by counting rather than guessing.
pub fn map(sheet: &Sheet) -> String {
    let mut out = String::new();
    for (row, names) in sheet.names.chunks(sheet.columns as usize).enumerate() {
        out.push_str(&format!("  row {row}:"));
        for name in names {
            out.push_str(&format!(" {name}"));
        }
        out.push('\n');
    }
    out
}
