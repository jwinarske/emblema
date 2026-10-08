//! What the device says about exporting each format and modifier.
//!
//! `DmaBufSupport::import`/`export` are read off the extension list, which
//! says the entry points exist and not that any format can be handed over.
//! On an i.MX8MP that is wrong in the direction that matters: the device
//! advertises three renderable formats and refuses `B8G8R8A8_UNORM` as a
//! dma-buf, which surfaces as four validation errors deep inside
//! `an_exported_image_can_still_be_rendered_into`.
//!
//! A filter was written for that and reverted, because it took the same board
//! from three render formats to zero -- and `XR30` at the linear modifier
//! demonstrably does export there. So the obvious query over-rejects
//! something that works, and the next step is to print what the device
//! actually answers rather than to guess at a predicate again.
//!
//! Everything here is a physical-device query, so there is no logical device
//! and nothing to tear down. Run it on a board and read the table.

// An example is a program, and it reports by printing.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ash::vk;

/// The formats `render_formats` considers, in its order.
const FORMATS: &[(&str, vk::Format)] = &[
    ("Bgra8Unorm", vk::Format::B8G8R8A8_UNORM),
    ("Rgba8Unorm", vk::Format::R8G8B8A8_UNORM),
    ("Rgb10A2Unorm", vk::Format::A2B10G10R10_UNORM_PACK32),
];

/// The usage `create_exportable_texture` asks for. The answer depends on it,
/// which is half the reason this is worth printing rather than assuming.
const USAGE: vk::ImageUsageFlags = vk::ImageUsageFlags::from_raw(
    vk::ImageUsageFlags::COLOR_ATTACHMENT.as_raw() | vk::ImageUsageFlags::TRANSFER_SRC.as_raw(),
);

fn main() {
    // SAFETY: the loader and instance live for the whole program, and every
    // chained structure below outlives the call it is passed to.
    unsafe {
        let entry = ash::Entry::load().expect("loading Vulkan");
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
        let instance = entry
            .create_instance(
                &vk::InstanceCreateInfo::default().application_info(&app),
                None,
            )
            .expect("create_instance");

        for pd in instance
            .enumerate_physical_devices()
            .expect("enumerate_physical_devices")
        {
            let props = instance.get_physical_device_properties(pd);
            println!(
                "\n== {}",
                std::ffi::CStr::from_ptr(props.device_name.as_ptr()).to_string_lossy()
            );

            for (name, format) in FORMATS {
                // The modifiers the device advertises for this format, with
                // the tiling features `render_formats` filters on today.
                let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
                let mut properties = vk::FormatProperties2::default().push_next(&mut list);
                instance.get_physical_device_format_properties2(pd, *format, &mut properties);
                let count = list.drm_format_modifier_count as usize;
                if count == 0 {
                    println!("  {name:14} no modifiers advertised");
                    continue;
                }
                let mut entries = vec![vk::DrmFormatModifierPropertiesEXT::default(); count];
                list.p_drm_format_modifier_properties = entries.as_mut_ptr();
                let mut properties = vk::FormatProperties2::default().push_next(&mut list);
                instance.get_physical_device_format_properties2(pd, *format, &mut properties);

                for entry in entries.iter().take(count) {
                    let renderable = entry
                        .drm_format_modifier_tiling_features
                        .contains(vk::FormatFeatureFlags::COLOR_ATTACHMENT);

                    // The external query, exactly as the reverted filter made
                    // it. What it answers is the thing to look at.
                    let mut modifier_info =
                        vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
                            .drm_format_modifier(entry.drm_format_modifier)
                            .sharing_mode(vk::SharingMode::EXCLUSIVE);
                    let mut external = vk::PhysicalDeviceExternalImageFormatInfo::default()
                        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
                    let info = vk::PhysicalDeviceImageFormatInfo2::default()
                        .format(*format)
                        .ty(vk::ImageType::TYPE_2D)
                        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
                        .usage(USAGE)
                        .push_next(&mut external)
                        .push_next(&mut modifier_info);
                    let mut external_properties = vk::ExternalImageFormatProperties::default();
                    let mut out =
                        vk::ImageFormatProperties2::default().push_next(&mut external_properties);
                    let answer =
                        instance.get_physical_device_image_format_properties2(pd, &info, &mut out);

                    let verdict = match answer {
                        Err(e) => format!("query refused: {e:?}"),
                        Ok(()) => {
                            let m = external_properties.external_memory_properties;
                            format!(
                                "features {:?}, compatible {:?}, exportable-from {:?}",
                                m.external_memory_features,
                                m.compatible_handle_types,
                                m.export_from_imported_handle_types
                            )
                        }
                    };
                    println!(
                        "  {name:14} modifier {:#018x}  renderable {renderable:5}  {verdict}",
                        entry.drm_format_modifier
                    );
                }
            }
        }
        instance.destroy_instance(None);
    }
}
