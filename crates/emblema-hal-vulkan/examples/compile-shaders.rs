//! Hand every shader module to a driver's pipeline compiler, with no renderer in the way.
//!
//! An instance, a device, `vkCreateShaderModule` and one `vkCreateGraphicsPipelines` per
//! module. Nothing from this crate but the SPIR-V: no context, no batch, no pass. That is
//! the point -- a driver that crashes compiling a module crashes here too, and a report
//! naming this cannot be answered with "your renderer did something to it".
//!
//! Written for an i.MX8M Plus, whose Vivante driver segfaults inside `gcSPV_Decode` on one
//! of these. `docs/on-a-board.md` has what it found. It is kept because the next driver
//! that cannot compile these will be found the same way, and because the suite cannot ask
//! this question: a device that fails here fails every test too, which says nothing about
//! where.
//!
//! **The pipeline layout is the renderer's own, and that is not a detail.** The first
//! version of this used an empty layout, reasoning that less state meant a cleaner question
//! -- and it segfaulted on RADV, a driver that compiles these shaders every day. A pipeline
//! whose layout does not cover the resources its shaders declare is invalid usage, so a
//! crash there is the program's fault and proves nothing about any driver. The texture set
//! comes from `sampling::create_descriptor_layout` so it cannot drift; the material set is
//! one binding, mirrored from `materials::create_layout`, which is `pub(crate)`.
//!
//! Everything else is the smallest legal value: one color attachment, no vertex input, one
//! sample.
//!
//! ```sh
//! cargo run -p emblema-hal-vulkan --example compile-shaders
//! cargo run -p emblema-hal-vulkan --example compile-shaders -- solid effect
//! ```
//!
//! With two names it takes the vertex stage from the first and the fragment stage from the
//! second, which is how the crashing half of a module is found without editing a shader.

use ash::vk;
use std::ffi::CStr;

/// Every module this renderer compiles, with the names the sources give them.
fn modules() -> Vec<(&'static str, &'static [u32])> {
    vec![
        ("solid", emblema_shaders::SOLID_SPV),
        ("effect", emblema_shaders::EFFECT_SPV),
        ("effect-image", emblema_shaders::EFFECT_IMAGE_SPV),
        ("effect-mesh-uv", emblema_shaders::EFFECT_MESH_UV_SPV),
        ("effect-two-images", emblema_shaders::EFFECT_TWO_IMAGES_SPV),
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let all = modules();
    // Named rather than refused silently, and alongside what does exist, because the usual
    // reason to be here is a module this build does not have.
    let named = |name: &str| -> Option<&'static [u32]> {
        let found = all.iter().find(|(n, _)| *n == name).map(|(_, spv)| *spv);
        if found.is_none() {
            let names: Vec<&str> = all.iter().map(|(n, _)| *n).collect();
            eprintln!("no module {name:?}; this build has {names:?}");
        }
        found
    };

    let pairs: Vec<(String, &'static [u32], &'static [u32])> = match args.len() {
        0 => all
            .iter()
            .map(|(n, spv)| ((*n).to_string(), *spv, *spv))
            .collect(),
        1 => match named(&args[0]) {
            Some(spv) => vec![(args[0].clone(), spv, spv)],
            None => return,
        },
        _ => match (named(&args[0]), named(&args[1])) {
            (Some(vs), Some(fs)) => vec![(format!("{} vs + {} fs", args[0], args[1]), vs, fs)],
            _ => return,
        },
    };

    let entry = match unsafe { ash::Entry::load() } {
        Ok(entry) => entry,
        Err(e) => {
            // A skip, like every device test here: a machine with no loader cannot answer.
            eprintln!("skipping: no Vulkan loader ({e})");
            return;
        }
    };
    let app = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 1, 0));
    let instance = match unsafe {
        entry.create_instance(
            &vk::InstanceCreateInfo::default().application_info(&app),
            None,
        )
    } {
        Ok(instance) => instance,
        Err(e) => {
            eprintln!("skipping: no Vulkan instance ({e})");
            return;
        }
    };

    let devices = unsafe { instance.enumerate_physical_devices() }.unwrap_or_default();
    let Some(&physical) = devices.first() else {
        eprintln!("skipping: no Vulkan device");
        unsafe { instance.destroy_instance(None) };
        return;
    };
    let props = unsafe { instance.get_physical_device_properties(physical) };
    let name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) };
    println!(
        "{} -- Vulkan {}.{}.{}",
        name.to_string_lossy(),
        vk::api_version_major(props.api_version),
        vk::api_version_minor(props.api_version),
        vk::api_version_patch(props.api_version)
    );

    let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
    let Some(family) = families
        .iter()
        .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
    else {
        eprintln!("skipping: no graphics queue");
        unsafe { instance.destroy_instance(None) };
        return;
    };
    let priorities = [1.0f32];
    let queues = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(family as u32)
        .queue_priorities(&priorities)];
    let device = unsafe {
        instance.create_device(
            physical,
            &vk::DeviceCreateInfo::default().queue_create_infos(&queues),
            None,
        )
    }
    .expect("device");

    for (label, vs_spv, fs_spv) in pairs {
        // Flushed before the call, because the interesting outcome is the process not
        // reaching the line after it.
        print!("  {label:<22} {:>6} words ... ", vs_spv.len());
        use std::io::Write;
        let _ = std::io::stdout().flush();
        match compile(&device, vs_spv, fs_spv) {
            Ok(()) => println!("compiled"),
            Err(e) => println!("refused: {e:?}"),
        }
    }

    unsafe {
        device.destroy_device(None);
        instance.destroy_instance(None);
    }
}

/// One graphics pipeline from two entry points, built and thrown away.
fn compile(device: &ash::Device, vs_spv: &[u32], fs_spv: &[u32]) -> Result<(), vk::Result> {
    let vs = unsafe {
        device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(vs_spv), None)
    }?;
    let fs = if std::ptr::eq(vs_spv, fs_spv) {
        vs
    } else {
        unsafe {
            device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(fs_spv), None)
        }?
    };

    let color = [vk::AttachmentDescription::default()
        .format(vk::Format::B8G8R8A8_UNORM)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
    let refs = [vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
    let subpasses = [vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&refs)];
    let render_pass = unsafe {
        device.create_render_pass(
            &vk::RenderPassCreateInfo::default()
                .attachments(&color)
                .subpasses(&subpasses),
            None,
        )
    }?;
    // Set zero is the textures, set one the paint, in that order -- `render.rs` builds the
    // real one the same way. A layout may carry bindings a shader never mentions, which is
    // why one layout serves every module here.
    let textures = emblema_hal_vulkan::sampling::create_descriptor_layout(device)
        .expect("the renderer's own texture set layout");
    let material_bindings = [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
    let material = unsafe {
        device.create_descriptor_set_layout(
            &vk::DescriptorSetLayoutCreateInfo::default().bindings(&material_bindings),
            None,
        )
    }?;
    let set_layouts = [textures, material];
    let layout = unsafe {
        device.create_pipeline_layout(
            &vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts),
            None,
        )
    }?;

    let vs_name = c"vs_main";
    let fs_name = c"fs_main";
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vs)
            .name(vs_name),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(fs)
            .name(fs_name),
    ];
    let vertex = vk::PipelineVertexInputStateCreateInfo::default();
    let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewports = [vk::Viewport::default()
        .width(64.0)
        .height(64.0)
        .max_depth(1.0)];
    let scissors = [vk::Rect2D::default().extent(vk::Extent2D {
        width: 64,
        height: 64,
    })];
    let viewport = vk::PipelineViewportStateCreateInfo::default()
        .viewports(&viewports)
        .scissors(&scissors);
    let raster = vk::PipelineRasterizationStateCreateInfo::default().line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let blends = [vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)];
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blends);
    let info = [vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex)
        .input_assembly_state(&assembly)
        .viewport_state(&viewport)
        .rasterization_state(&raster)
        .multisample_state(&multisample)
        .color_blend_state(&blend)
        .layout(layout)
        .render_pass(render_pass)
        .subpass(0)];

    let outcome =
        unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &info, None) };
    unsafe {
        if let Ok(pipelines) = &outcome {
            for &p in pipelines {
                device.destroy_pipeline(p, None);
            }
        }
        device.destroy_pipeline_layout(layout, None);
        device.destroy_descriptor_set_layout(material, None);
        device.destroy_descriptor_set_layout(textures, None);
        device.destroy_render_pass(render_pass, None);
        if fs != vs {
            device.destroy_shader_module(fs, None);
        }
        device.destroy_shader_module(vs, None);
    }
    outcome.map(|_| ()).map_err(|(_, e)| e)
}
