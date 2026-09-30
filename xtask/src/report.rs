//! What this machine's devices can do, gathered and printed.
//!
//! The project spans desktop discrete GPUs down to embedded SoCs driving panels
//! directly, and the layers above the HAL branch on capabilities rather than on
//! which backend is in play. So the first thing anyone bringing up a board
//! needs is the answer to "what does this one report", and the first thing
//! anyone reading a bug report needs is the same answer for the machine it came
//! from.
//!
//! Every device is reported, not only the one a preference would pick. A
//! machine with a discrete GPU and a software rasterizer has two, they differ in
//! ways that decide which paths run — an extension can sit on one and not the
//! other — and a report that showed one would answer the wrong question.

use emblema_hal::{Capabilities, HalContext};
use emblema_hal_gles::{DisplayTarget, GlesContext};
use emblema_hal_vulkan::{DevicePreference, VulkanContext};

/// One device, and what it reports.
pub struct Device {
    /// How a caller would ask for this device again.
    pub selector: String,
    pub backend: &'static str,
    pub capabilities: Capabilities,
}

/// Every device this build can reach.
///
/// A backend that cannot start is absent rather than an error: a machine with
/// no Vulkan driver is a machine this reports the GLES side of, and refusing to
/// print anything would be the least useful response to the question being
/// asked.
pub fn gather() -> Vec<Device> {
    let mut devices = Vec::new();

    // By index rather than by preference, so a machine with several gets all of
    // them. Enumeration stops at the first index that does not exist, which is
    // also how the backend reports running out.
    for index in 0..16 {
        let Ok(ctx) = VulkanContext::new(DevicePreference::Index(index)) else {
            break;
        };
        devices.push(Device {
            selector: format!("vulkan:{index}"),
            backend: "vulkan",
            capabilities: HalContext::capabilities(&ctx).clone(),
        });
    }

    if let Ok(ctx) = GlesContext::new(DisplayTarget::Surfaceless) {
        devices.push(Device {
            selector: "gles".into(),
            backend: "gles",
            capabilities: HalContext::capabilities(&ctx).clone(),
        });
    }

    devices
}

/// A human-readable report.
pub fn text(devices: &[Device]) -> String {
    if devices.is_empty() {
        return "no rendering device could be created on this machine\n".into();
    }

    let mut out = String::new();
    for device in devices {
        let c = &device.capabilities;
        out.push_str(&format!("{} ({})\n", device.selector, device.backend));
        out.push_str(&format!("  device            {}\n", c.device_name));
        out.push_str(&format!("  driver            {}\n", c.driver_name));
        out.push_str(&format!("  max texture       {}\n", c.max_texture_size));

        // Printed as the counts themselves rather than as a mask, because a
        // mask is a thing to decode and this is a thing to read.
        let samples: Vec<String> = [1, 2, 4, 8, 16]
            .into_iter()
            .filter(|n| c.sample_counts.supports(*n))
            .map(|n| n.to_string())
            .collect();
        out.push_str(&format!("  sample counts     {}\n", samples.join(", ")));
        out.push_str(&format!(
            "  advanced blend    {}\n",
            yes_no(c.advanced_blend)
        ));
        out.push_str(&format!(
            "  dma-buf           import {}, export {}, modifiers {}\n",
            yes_no(c.dma_buf.import),
            yes_no(c.dma_buf.export),
            yes_no(c.dma_buf.modifiers)
        ));
        out.push_str(&format!(
            "  sync_file         export {}, import {}\n",
            yes_no(c.sync.export_sync_file),
            yes_no(c.sync.import_sync_file)
        ));
        // The two conclusions the layers above actually draw from all of the
        // above, stated rather than left to be recomputed by whoever is reading.
        out.push_str(&format!(
            "  scanout           {}\n",
            yes_no(c.supports_scanout())
        ));
        out.push_str(&format!(
            "  float targets     {}\n",
            if c.float_render_targets { "yes" } else { "no" }
        ));
        out.push_str(&format!("  render formats    {}\n", c.render_formats.len()));
        out.push('\n');
    }
    out
}

/// The same report as JSON, for a machine to consume.
///
/// Written out by hand rather than through a serialization crate. The shape is
/// flat and fixed, this is the only thing in the tree that needs it, and a
/// dependency taken for one report would be carried by every build.
pub fn json(devices: &[Device]) -> String {
    let mut out = String::from("{\n  \"devices\": [\n");
    for (i, device) in devices.iter().enumerate() {
        let c = &device.capabilities;
        let samples: Vec<String> = [1, 2, 4, 8, 16]
            .into_iter()
            .filter(|n| c.sample_counts.supports(*n))
            .map(|n| n.to_string())
            .collect();
        out.push_str("    {\n");
        out.push_str(&format!(
            "      \"selector\": {},\n",
            quote(&device.selector)
        ));
        out.push_str(&format!("      \"backend\": {},\n", quote(device.backend)));
        out.push_str(&format!("      \"device\": {},\n", quote(&c.device_name)));
        out.push_str(&format!("      \"driver\": {},\n", quote(&c.driver_name)));
        out.push_str(&format!(
            "      \"maxTextureSize\": {},\n",
            c.max_texture_size
        ));
        out.push_str(&format!(
            "      \"sampleCounts\": [{}],\n",
            samples.join(", ")
        ));
        out.push_str(&format!("      \"advancedBlend\": {},\n", c.advanced_blend));
        out.push_str(&format!(
            "      \"dmaBuf\": {{ \"import\": {}, \"export\": {}, \"modifiers\": {} }},\n",
            c.dma_buf.import, c.dma_buf.export, c.dma_buf.modifiers
        ));
        out.push_str(&format!(
            "      \"syncFile\": {{ \"export\": {}, \"import\": {} }},\n",
            c.sync.export_sync_file, c.sync.import_sync_file
        ));
        out.push_str(&format!("      \"scanout\": {},\n", c.supports_scanout()));
        out.push_str(&format!(
            "      \"renderFormats\": {}\n",
            c.render_formats.len()
        ));
        out.push_str(if i + 1 == devices.len() {
            "    }\n"
        } else {
            "    },\n"
        });
    }
    out.push_str("  ]\n}\n");
    out
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

/// A JSON string literal.
///
/// Device and driver names come from a driver and are not this project's to
/// trust: a quote or a backslash in one would otherwise produce output that
/// does not parse, and whoever hit it would be debugging their JSON reader.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Everything below a space has to be escaped, and the ones without
            // a short form take the numeric one.
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_escapes_what_would_otherwise_break_the_output() {
        assert_eq!(quote("plain"), "\"plain\"");
        assert_eq!(quote(r#"a "quoted" name"#), r#""a \"quoted\" name""#);
        assert_eq!(quote(r"back\slash"), r#""back\\slash""#);
        assert_eq!(quote("two\nlines"), "\"two\\nlines\"");
        // A control character with no short form takes the numeric escape
        // rather than being passed through as a raw byte.
        assert_eq!(quote("\u{1}"), "\"\\u0001\"");
    }

    #[test]
    fn quoting_leaves_text_that_needs_nothing_alone() {
        // Driver names carry parentheses, commas and non-ASCII, and escaping
        // those would be as wrong as failing to escape a quote.
        let name = "AMD Radeon (RADV), Mesa 24.1 — µarch";
        assert_eq!(quote(name), format!("\"{name}\""));
    }

    #[test]
    fn a_machine_with_no_device_says_so_rather_than_printing_nothing() {
        // The honest answer to "what can this machine do" is sometimes
        // "nothing", and an empty report reads as a broken tool.
        let text = text(&[]);
        assert!(text.contains("no rendering device"), "{text}");
    }

    #[test]
    fn an_empty_report_is_still_valid_json() {
        assert_eq!(json(&[]), "{\n  \"devices\": [\n  ]\n}\n");
    }
}

#[cfg(test)]
mod device_tests {
    use super::*;

    #[test]
    fn a_report_of_this_machine_is_readable_and_parses() {
        let devices = gather();
        if devices.is_empty() {
            eprintln!("skipping: no rendering device on this machine");
            return;
        }

        // Every device appears in both renderings, and each names itself in a
        // way a caller could use to ask for it again. A report that identified
        // devices only by driver-supplied name would not survive two identical
        // cards.
        let text = text(&devices);
        let json = json(&devices);
        for device in &devices {
            assert!(
                text.contains(&device.selector),
                "the text report omits {}",
                device.selector
            );
            assert!(
                json.contains(&device.selector),
                "the JSON report omits {}",
                device.selector
            );
        }

        // Selectors are unique, or "ask for it again" is ambiguous.
        let mut selectors: Vec<&str> = devices.iter().map(|d| d.selector.as_str()).collect();
        selectors.sort_unstable();
        let count = selectors.len();
        selectors.dedup();
        assert_eq!(selectors.len(), count, "two devices share a selector");

        // Structurally sound rather than merely non-empty: matched braces and
        // brackets is the cheapest check that catches a missing separator,
        // which is the way hand-written JSON goes wrong.
        let balanced = |open: char, close: char| {
            json.chars().filter(|c| *c == open).count()
                == json.chars().filter(|c| *c == close).count()
        };
        assert!(balanced('{', '}') && balanced('[', ']'), "{json}");
        assert!(
            !json.contains(",\n  ]"),
            "a trailing comma before the closing bracket:\n{json}"
        );
    }

    #[test]
    fn every_device_a_selector_names_can_be_asked_for_again() {
        // The selector is the report's whole practical value: it is what a
        // person types after reading it. One that did not round-trip would send
        // them to a different device than the one whose capabilities they read.
        for device in gather() {
            let Some(index) = device.selector.strip_prefix("vulkan:") else {
                continue;
            };
            let index: usize = index.parse().expect("a vulkan selector carries an index");
            let ctx = VulkanContext::new(DevicePreference::Index(index))
                .expect("the selector named a device that does not exist");
            assert_eq!(
                HalContext::capabilities(&ctx).device_name,
                device.capabilities.device_name,
                "{} named a different device the second time",
                device.selector
            );
        }
    }
}
