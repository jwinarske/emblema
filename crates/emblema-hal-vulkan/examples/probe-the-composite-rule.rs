//! Which SPIR-V shape crashes a driver's pipeline compiler.
//!
//! Written for one question on one device. An i.MX8MP's Vivante GC7000UL
//! segfaults inside `vkCreateComputePipelines` and
//! `vkCreateGraphicsPipelines`, in `VIR_Shader_CompositeConstruct`. The
//! candidate rule was that it needs an `OpCompositeConstruct` whose operand is
//! an `OpFunctionParameter` result, inside a non-entry-point function --
//! narrow enough that a shader could be written to avoid it.
//!
//! `docs/on-a-board.md` records that avoiding exactly that in `solid.wgsl`
//! changed nothing, which leaves at least two rules fitting the same evidence.
//! One variable at a time is what tells them apart, and a renderer's shader
//! has fifty-one composite constructs across eighteen functions, so it cannot.
//!
//! Each variant below differs from its neighbor in one thing:
//!
//! | variant | called function | composite construct | operand |
//! |---|---|---|---|
//! | `entry-only` | no | in the entry point | a loaded global |
//! | `called-no-construct` | yes | none | -- |
//! | `called-no-params` | yes, takes nothing | in the callee | a loaded global |
//! | `called-loaded-param` | yes, takes a pointer | in the callee | a load *of* the parameter |
//! | `called-param-operand` | yes, takes a value | in the callee | the parameter |
//!
//! So: `entry-only` and `called-no-construct` passing with
//! `called-param-operand` failing says the rule is about parameters.
//! `called-no-params` failing too says it is about called functions, and no
//! shader-source fix short of inlining everything exists.
//!
//! A crash takes the process with it, so one variant runs per invocation and
//! the exit status is the result. With no argument it lists them.
//!
//! ```text
//! for v in entry-only called-no-construct called-no-params called-param-operand; do
//!     probe-the-composite-rule $v; echo "$v -> $?"
//! done
//! ```
//!
//! Compute rather than graphics: the same compiler, a tenth of the setup, and
//! nothing about a render pass or a vertex layout to be wrong about.

// An example is a program. It reports by exiting, and the failures it cares
// about are a driver's rather than its own.
#![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]

use ash::vk;

/// Each variant, as the one thing it is there to vary.
///
/// All four write through the same storage buffer, so the construct cannot be
/// dropped as dead before it reaches the driver -- which would make a variant
/// pass for the wrong reason and is the easiest way to get a null result here.
const VARIANTS: &[(&str, &str)] = &[
    (
        "entry-only",
        r#"
@group(0) @binding(0) var<storage, read_write> out: array<f32>;

@compute @workgroup_size(1)
fn main() {
    let v = vec2<f32>(out[1], 0.5);
    out[0] = v.x + v.y;
}
"#,
    ),
    (
        "called-no-construct",
        r#"
@group(0) @binding(0) var<storage, read_write> out: array<f32>;

fn scaled(x: f32) -> f32 {
    return x * 2.0;
}

@compute @workgroup_size(1)
fn main() {
    out[0] = scaled(out[1]);
}
"#,
    ),
    (
        "called-no-params",
        r#"
@group(0) @binding(0) var<storage, read_write> out: array<f32>;

fn built() -> vec2<f32> {
    return vec2<f32>(out[1], 0.5);
}

@compute @workgroup_size(1)
fn main() {
    let v = built();
    out[0] = v.x + v.y;
}
"#,
    ),
    (
        "called-loaded-param",
        r#"
@group(0) @binding(0) var<storage, read_write> out: array<f32>;

fn built(p: ptr<function, f32>) -> vec2<f32> {
    return vec2<f32>(*p, 0.5);
}

@compute @workgroup_size(1)
fn main() {
    var x = out[1];
    let v = built(&x);
    out[0] = v.x + v.y;
}
"#,
    ),
    (
        "called-param-operand",
        r#"
@group(0) @binding(0) var<storage, read_write> out: array<f32>;

fn built(x: f32) -> vec2<f32> {
    return vec2<f32>(x, 0.5);
}

@compute @workgroup_size(1)
fn main() {
    let v = built(out[1]);
    out[0] = v.x + v.y;
}
"#,
    ),
];

/// What the variant's module actually contains, read back out of the SPIR-V.
///
/// Printed rather than asserted, because the claim "this variant has a
/// composite construct in a called function" is about what naga emitted and
/// not about what the WGSL looks like. A variant that stopped emitting one --
/// through inlining, or constant folding -- would pass and mean nothing.
fn describe(words: &[u32]) -> String {
    let (mut entries, mut params, mut functions) = (Vec::new(), Vec::new(), Vec::new());
    let (mut in_entry, mut in_called) = (0usize, 0usize);
    let mut from_param = 0usize;
    let mut i = 5;
    let mut current = 0u32;
    // Entry points are declared before any function body, so one pass is
    // enough to know whether the function a construct sits in is one.
    while i < words.len() {
        let (count, op) = ((words[i] >> 16) as usize, words[i] & 0xFFFF);
        if count == 0 {
            break;
        }
        match op {
            15 => entries.push(words[i + 2]), // OpEntryPoint
            54 => {
                current = words[i + 2]; // OpFunction
                functions.push(current);
            }
            55 => params.push(words[i + 2]), // OpFunctionParameter
            80 => {
                // OpCompositeConstruct
                if entries.contains(&current) {
                    in_entry += 1;
                } else {
                    in_called += 1;
                }
                if words[i + 3..i + count].iter().any(|o| params.contains(o)) {
                    from_param += 1;
                }
            }
            _ => {}
        }
        i += count;
    }
    format!(
        "{} functions ({} entry), {} parameters, {} construct(s) in entry and \
         {} in called functions, {} taking a parameter",
        functions.len(),
        entries.len(),
        params.len(),
        in_entry,
        in_called,
        from_param
    )
}

fn spirv(source: &str) -> Vec<u32> {
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("parsing the variant:\n{}", e.emit_to_string(source)));
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("validating the variant: {e}"));
    naga::back::spv::write_vec(&module, &info, &naga::back::spv::Options::default(), None)
        .expect("translating the variant")
}

fn main() -> std::process::ExitCode {
    let Some(wanted) = std::env::args().nth(1) else {
        println!("variants:");
        for (name, source) in VARIANTS {
            println!("  {name:22} {}", describe(&spirv(source)));
        }
        return std::process::ExitCode::SUCCESS;
    };
    let Some((name, source)) = VARIANTS.iter().find(|(n, _)| *n == wanted) else {
        eprintln!("no variant named {wanted}");
        return std::process::ExitCode::FAILURE;
    };

    let words = spirv(source);
    println!("{name}: {}", describe(&words));

    // SAFETY: the loader lives for the rest of the program, and every object
    // below is created from the one before it and destroyed in reverse.
    unsafe {
        let entry = ash::Entry::load().expect("loading Vulkan");
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
        let instance = entry
            .create_instance(
                &vk::InstanceCreateInfo::default().application_info(&app),
                None,
            )
            .expect("create_instance");
        let physical = *instance
            .enumerate_physical_devices()
            .expect("enumerate_physical_devices")
            .first()
            .expect("a Vulkan device");
        let properties = instance.get_physical_device_properties(physical);
        println!(
            "device: {}",
            std::ffi::CStr::from_ptr(properties.device_name.as_ptr()).to_string_lossy()
        );

        let family = instance
            .get_physical_device_queue_family_properties(physical)
            .iter()
            .position(|f| f.queue_flags.contains(vk::QueueFlags::COMPUTE))
            .expect("a compute queue") as u32;
        let priorities = [1.0f32];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family)
            .queue_priorities(&priorities)];
        let device = instance
            .create_device(
                physical,
                &vk::DeviceCreateInfo::default().queue_create_infos(&queues),
                None,
            )
            .expect("create_device");

        let module = device
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)
            .expect("create_shader_module");
        // The module always survives: this driver decodes SPIR-V when a
        // pipeline is built, not when a module is made, which is why the
        // crash lands where it does.
        println!("module created");

        let binding = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)];
        let set_layout = device
            .create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&binding),
                None,
            )
            .expect("create_descriptor_set_layout");
        let set_layouts = [set_layout];
        let layout = device
            .create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts),
                None,
            )
            .expect("create_pipeline_layout");

        let name = std::ffi::CString::new("main").unwrap();
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module)
            .name(&name);
        let info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(layout);

        println!("creating the pipeline");
        let pipelines = device.create_compute_pipelines(vk::PipelineCache::null(), &[info], None);
        match pipelines {
            Ok(pipelines) => {
                println!("pipeline created");
                for pipeline in pipelines {
                    device.destroy_pipeline(pipeline, None);
                }
            }
            Err((_, e)) => {
                println!("refused: {e:?}");
                device.destroy_pipeline_layout(layout, None);
                device.destroy_descriptor_set_layout(set_layout, None);
                device.destroy_shader_module(module, None);
                device.destroy_device(None);
                instance.destroy_instance(None);
                return std::process::ExitCode::FAILURE;
            }
        }

        device.destroy_pipeline_layout(layout, None);
        device.destroy_descriptor_set_layout(set_layout, None);
        device.destroy_shader_module(module, None);
        device.destroy_device(None);
        instance.destroy_instance(None);
    }
    std::process::ExitCode::SUCCESS
}
