use anyhow::{Context, Result};
use ash::vk;
use glam::{Mat4, Quat, Vec3, Vec4};
use openxr as xr;

use crate::{
    gpu::SharedImage,
    panel::{PanelGeometry, PanelPose},
};

pub(crate) const FALLBACK_FLOOR_Y: f32 = -1.3;

const SHADER: &str = r#"
const FLOOR_REFLECTANCE: f32 = 0.18;
const FLOOR_ROUGHNESS: f32 = 0.25;
const FLOOR_RAY_COUNT: u32 = 4u;
const FLOOR_NOISE_CELL_SIZE: f32 = 0.002;
const PI: f32 = 3.14159265;
const_assert FLOOR_RAY_COUNT > 0u;
const_assert FLOOR_ROUGHNESS >= 0.0 && FLOOR_ROUGHNESS <= 1.0;
const_assert FLOOR_REFLECTANCE >= 0.0 && FLOOR_REFLECTANCE <= 1.0;

struct Transform {
    matrix: mat4x4<f32>,
    emitter_center_width: vec4<f32>,
    emitter_right_height: vec4<f32>,
    emitter_up: vec4<f32>,
    eye_position: vec4<f32>,
}
var<immediate> transform: Transform;
@group(0) @binding(0) var panel: texture_2d<f32>;
@group(0) @binding(1) var filtering: sampler;
struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}
@vertex fn vertex(@builtin(vertex_index) index: u32) -> Vertex {
    let corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 0.0),
        vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 1.0));
    let uv = corners[index];
    var result: Vertex;
    result.position = transform.matrix * vec4(uv.x - 0.5, 0.5 - uv.y, 0.0, 1.0);
    result.uv = uv;
    return result;
}
@fragment fn window(input: Vertex) -> @location(0) vec4<f32> {
    let color = textureSample(panel, filtering, input.uv);
    if color.a < 0.001 { discard; }
    return color;
}
@fragment fn cursor(input: Vertex) -> @location(0) vec4<f32> {
    let stroke = max(vec2(0.5 / 21.0), fwidth(input.uv) * 0.75);
    if all(abs(input.uv - vec2(0.5)) > stroke) { discard; }
    return vec4(1.0, 0.9131, 0.0, 1.0);
}
struct FloorVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) world: vec3<f32>,
}
@vertex fn floor_vertex(@builtin(vertex_index) index: u32) -> FloorVertex {
    let corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 0.0),
        vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 1.0));
    let world = vec3((corners[index].x - 0.5) * 60.0, transform.emitter_up.w,
        (corners[index].y - 0.5) * 60.0);
    var result: FloorVertex;
    result.position = transform.matrix * vec4(world, 1.0);
    result.world = world;
    return result;
}
fn sample_ggx_visible_normal(view: vec3<f32>, sample_uv: vec2<f32>, alpha: f32) -> vec3<f32> {
    let stretched = normalize(vec3(alpha * view.x, alpha * view.z, view.y));
    let tangent_length_squared = dot(stretched.xy, stretched.xy);
    var tangent = vec3(1.0, 0.0, 0.0);
    if tangent_length_squared > 0.0 {
        tangent = vec3(-stretched.y, stretched.x, 0.0) * inverseSqrt(tangent_length_squared);
    }
    let bitangent = cross(stretched, tangent);
    let radius = sqrt(sample_uv.x);
    let angle = 2.0 * PI * sample_uv.y;
    let disk_x = radius * cos(angle);
    let blend = 0.5 * (1.0 + stretched.z);
    let disk_y = (1.0 - blend) * sqrt(max(0.0, 1.0 - disk_x * disk_x))
        + blend * radius * sin(angle);
    let hemisphere = tangent * disk_x + bitangent * disk_y
        + stretched * sqrt(max(0.0, 1.0 - disk_x * disk_x - disk_y * disk_y));
    let local_normal = normalize(vec3(alpha * hemisphere.xy, max(0.0, hemisphere.z)));
    return vec3(local_normal.x, local_normal.z, local_normal.y);
}
fn ggx_masking(cosine: f32, alpha_squared: f32) -> f32 {
    return 2.0 * cosine / (cosine + sqrt(alpha_squared + (1.0 - alpha_squared) * cosine * cosine));
}
fn sample_hash(value: u32) -> u32 {
    let state = value * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}
fn sample_jitter(position: vec3<f32>, index: u32) -> vec2<f32> {
    let cell = bitcast<vec2<u32>>(vec2<i32>(round(position.xz / FLOOR_NOISE_CELL_SIZE)));
    let seed = sample_hash(cell.x ^ sample_hash(cell.y) ^ sample_hash(index + 0x9e3779b9u));
    let random = vec2(sample_hash(seed), sample_hash(seed ^ 0x85ebca6bu));
    return vec2<f32>(random >> vec2(8u)) / 16777216.0;
}
@fragment fn floor_light(input: FloorVertex) -> @location(0) vec4<f32> {
    let center = transform.emitter_center_width.xyz;
    let width = transform.emitter_center_width.w;
    let right = transform.emitter_right_height.xyz;
    let height = transform.emitter_right_height.w;
    let up = transform.emitter_up.xyz;
    let normal = cross(right, up);
    let to_eye = transform.eye_position.xyz - input.world;
    if to_eye.y <= 0.0 { return vec4(0.0); }
    let view = normalize(to_eye);
    let alpha = max(0.001, FLOOR_ROUGHNESS * FLOOR_ROUGHNESS);
    let alpha_squared = alpha * alpha;
    let view_masking = ggx_masking(view.y, alpha_squared);
    let rows = max(1u, u32(sqrt(f32(FLOOR_RAY_COUNT))));
    let short_row_count = FLOOR_RAY_COUNT / rows;
    let long_rows = FLOOR_RAY_COUNT % rows;
    var sum = vec3(0.0);
    for (var index = 0u; index < FLOOR_RAY_COUNT; index += 1u) {
        let row = index % rows;
        let columns = short_row_count + select(0u, 1u, row < long_rows);
        let row_offset = row * short_row_count + min(row, long_rows);
        let jitter = sample_jitter(input.world, index);
        let sample_uv = vec2(
            (f32(index / rows) + jitter.x) / f32(columns),
            (f32(row_offset) + jitter.y * f32(columns)) / f32(FLOOR_RAY_COUNT));
        let half_vector = sample_ggx_visible_normal(view, sample_uv, alpha);
        let ray = reflect(-view, half_vector);
        let denominator = dot(ray, normal);
        if denominator >= -0.00001 || ray.y <= 0.0 { continue; }
        let distance = dot(center - input.world, normal) / denominator;
        if distance <= 0.0 { continue; }
        let hit = input.world + ray * distance - center;
        let uv = vec2(dot(hit, right) / width + 0.5, 0.5 - dot(hit, up) / height);
        if any(uv < vec2(0.0)) || any(uv > vec2(1.0)) { continue; }
        let color = textureSampleLevel(panel, filtering, uv, 0.0);
        let view_half = clamp(dot(view, half_vector), 0.0, 1.0);
        let fresnel = FLOOR_REFLECTANCE + (1.0 - FLOOR_REFLECTANCE) * pow(1.0 - view_half, 5.0);
        let light_masking = ggx_masking(ray.y, alpha_squared);
        let weight = fresnel * light_masking
            / (view_masking + light_masking - view_masking * light_masking);
        sum += color.rgb * weight;
    }
    return vec4(sum / f32(FLOOR_RAY_COUNT), 0.0);
}
"#;

fn shader(entry: &str, stage: naga::ShaderStage) -> Result<Vec<u32>> {
    let module = naga::front::wgsl::parse_str(SHADER)
        .map_err(|error| anyhow::anyhow!(error.emit_to_string(SHADER)))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::IMMEDIATES,
    )
    .validate(&module)?;
    let options = naga::back::spv::Options {
        flags: naga::back::spv::WriterFlags::empty(),
        ..Default::default()
    };
    Ok(naga::back::spv::write_vec(
        &module,
        &info,
        &options,
        Some(&naga::back::spv::PipelineOptions {
            shader_stage: stage,
            entry_point: entry.into(),
        }),
    )?)
}

pub(crate) fn view_projection(view: &xr::View) -> Mat4 {
    let left = view.fov.angle_left.tan();
    let right = view.fov.angle_right.tan();
    let down = view.fov.angle_down.tan();
    let up = view.fov.angle_up.tan();
    let near = 0.05;
    let far = 100.0;
    let projection = Mat4::from_cols(
        Vec4::new(2.0 / (right - left), 0.0, 0.0, 0.0),
        Vec4::new(0.0, -2.0 / (up - down), 0.0, 0.0),
        Vec4::new(
            (right + left) / (right - left),
            -(up + down) / (up - down),
            far / (near - far),
            -1.0,
        ),
        Vec4::new(0.0, 0.0, far * near / (near - far), 0.0),
    );
    let pose = view.pose;
    let camera = Mat4::from_rotation_translation(
        Quat::from_xyzw(
            pose.orientation.x,
            pose.orientation.y,
            pose.orientation.z,
            pose.orientation.w,
        ),
        Vec3::new(pose.position.x, pose.position.y, pose.position.z),
    );
    projection * camera.inverse()
}

fn model(geometry: PanelGeometry) -> Mat4 {
    Mat4::from_scale_rotation_translation(
        Vec3::new(
            geometry.pose.width_m,
            geometry.pose.width_m * geometry.logical_size.h as f32 / geometry.logical_size.w as f32,
            1.0,
        ),
        geometry.pose.orientation(),
        geometry.pose.center,
    )
}

fn image_range(aspect: vk::ImageAspectFlags) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(aspect)
        .level_count(1)
        .layer_count(1)
}

fn image_view(
    device: &ash::Device,
    image: vk::Image,
    format: vk::Format,
    aspect: vk::ImageAspectFlags,
) -> Result<vk::ImageView> {
    Ok(unsafe {
        device.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(image_range(aspect)),
            None,
        )
    }?)
}

pub(crate) struct PanelTexture {
    device: ash::Device,
    pub shared: SharedImage,
    view: vk::ImageView,
    pool: vk::DescriptorPool,
    descriptor: vk::DescriptorSet,
}

impl PanelTexture {
    pub fn new(renderer: &SceneRenderer, shared: SharedImage) -> Result<Self> {
        let device = &renderer.device;
        let mut texture = Self {
            device: device.clone(),
            shared,
            view: vk::ImageView::null(),
            pool: vk::DescriptorPool::null(),
            descriptor: vk::DescriptorSet::null(),
        };
        texture.view = image_view(
            device,
            texture.shared.image,
            vk::Format::R8G8B8A8_SRGB,
            vk::ImageAspectFlags::COLOR,
        )?;
        let sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: 1,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLER,
                descriptor_count: 1,
            },
        ];
        unsafe {
            texture.pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&sizes),
                None,
            )?;
            texture.descriptor = device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(texture.pool)
                    .set_layouts(&[renderer.descriptor_layout]),
            )?[0];
            let images = [vk::DescriptorImageInfo::default()
                .image_view(texture.view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let samplers = [vk::DescriptorImageInfo::default().sampler(renderer.sampler)];
            device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(texture.descriptor)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(&images),
                    vk::WriteDescriptorSet::default()
                        .dst_set(texture.descriptor)
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::SAMPLER)
                        .image_info(&samplers),
                ],
                &[],
            );
        }
        Ok(texture)
    }

    pub unsafe fn ownership(&self, command: vk::CommandBuffer, queue_family: u32, acquire: bool) {
        let (
            old,
            new,
            source,
            destination,
            source_access,
            destination_access,
            source_stage,
            destination_stage,
        ) = if acquire {
            (
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::QUEUE_FAMILY_FOREIGN_EXT,
                queue_family,
                vk::AccessFlags::empty(),
                vk::AccessFlags::SHADER_READ,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
            )
        } else {
            (
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::ImageLayout::GENERAL,
                queue_family,
                vk::QUEUE_FAMILY_FOREIGN_EXT,
                vk::AccessFlags::SHADER_READ,
                vk::AccessFlags::empty(),
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            )
        };
        let barrier = vk::ImageMemoryBarrier::default()
            .image(self.shared.image)
            .subresource_range(image_range(vk::ImageAspectFlags::COLOR))
            .old_layout(old)
            .new_layout(new)
            .src_queue_family_index(source)
            .dst_queue_family_index(destination)
            .src_access_mask(source_access)
            .dst_access_mask(destination_access);
        unsafe {
            self.device.cmd_pipeline_barrier(
                command,
                source_stage,
                destination_stage,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }
    }
}

impl Drop for PanelTexture {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_descriptor_pool(self.pool, None);
            self.device.destroy_image_view(self.view, None);
        }
    }
}

pub(crate) struct RenderTarget {
    device: ash::Device,
    pub extent: vk::Extent2D,
    color_view: vk::ImageView,
    depth: vk::Image,
    depth_memory: vk::DeviceMemory,
    depth_view: vk::ImageView,
    framebuffer: vk::Framebuffer,
}

impl RenderTarget {
    pub fn new(renderer: &SceneRenderer, image: vk::Image, extent: vk::Extent2D) -> Result<Self> {
        let device = &renderer.device;
        let mut target = Self {
            device: device.clone(),
            extent,
            color_view: vk::ImageView::null(),
            depth: vk::Image::null(),
            depth_memory: vk::DeviceMemory::null(),
            depth_view: vk::ImageView::null(),
            framebuffer: vk::Framebuffer::null(),
        };
        target.color_view =
            image_view(device, image, renderer.format, vk::ImageAspectFlags::COLOR)?;
        unsafe {
            target.depth = device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::D32_SFLOAT)
                    .extent(vk::Extent3D {
                        width: extent.width,
                        height: extent.height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT),
                None,
            )?;
            let requirements = device.get_image_memory_requirements(target.depth);
            target.depth_memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(requirements.memory_type_bits.trailing_zeros()),
                None,
            )?;
            device.bind_image_memory(target.depth, target.depth_memory, 0)?;
            target.depth_view = image_view(
                device,
                target.depth,
                vk::Format::D32_SFLOAT,
                vk::ImageAspectFlags::DEPTH,
            )?;
            target.framebuffer = device.create_framebuffer(
                &vk::FramebufferCreateInfo::default()
                    .render_pass(renderer.render_pass)
                    .attachments(&[target.color_view, target.depth_view])
                    .width(extent.width)
                    .height(extent.height)
                    .layers(1),
                None,
            )?;
        }
        Ok(target)
    }
}

impl Drop for RenderTarget {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_framebuffer(self.framebuffer, None);
            self.device.destroy_image_view(self.depth_view, None);
            self.device.destroy_image(self.depth, None);
            self.device.free_memory(self.depth_memory, None);
            self.device.destroy_image_view(self.color_view, None);
        }
    }
}

pub(crate) struct SceneRenderer {
    device: ash::Device,
    format: vk::Format,
    render_pass: vk::RenderPass,
    descriptor_layout: vk::DescriptorSetLayout,
    sampler: vk::Sampler,
    layout: vk::PipelineLayout,
    window_pipeline: vk::Pipeline,
    cursor_pipeline: vk::Pipeline,
    floor_light_pipeline: vk::Pipeline,
}

impl SceneRenderer {
    pub fn new(device: &ash::Device, format: vk::Format) -> Result<Self> {
        let mut renderer = Self {
            device: device.clone(),
            format,
            render_pass: vk::RenderPass::null(),
            descriptor_layout: vk::DescriptorSetLayout::null(),
            sampler: vk::Sampler::null(),
            layout: vk::PipelineLayout::null(),
            window_pipeline: vk::Pipeline::null(),
            cursor_pipeline: vk::Pipeline::null(),
            floor_light_pipeline: vk::Pipeline::null(),
        };
        let attachments = [
            vk::AttachmentDescription::default()
                .format(format)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL),
            vk::AttachmentDescription::default()
                .format(vk::Format::D32_SFLOAT)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
        ];
        let color = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let depth = vk::AttachmentReference::default()
            .attachment(1)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let subpasses = [vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color)
            .depth_stencil_attachment(&depth)];
        let stages = vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
            | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
            | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS;
        let dependencies = [vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(stages)
            .dst_stage_mask(stages)
            .src_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            )
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            )];
        unsafe {
            renderer.render_pass = device.create_render_pass(
                &vk::RenderPassCreateInfo::default()
                    .attachments(&attachments)
                    .subpasses(&subpasses)
                    .dependencies(&dependencies),
                None,
            )?;
            let bindings = [
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            ];
            renderer.descriptor_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            renderer.sampler = device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )?;
            let constants = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .size(128)];
            renderer.layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&[renderer.descriptor_layout])
                    .push_constant_ranges(&constants),
                None,
            )?;
        }
        renderer.window_pipeline = renderer.pipeline("window")?;
        renderer.cursor_pipeline = renderer.pipeline("cursor")?;
        renderer.floor_light_pipeline = renderer.pipeline("floor_light")?;
        Ok(renderer)
    }

    fn pipeline(&self, fragment: &str) -> Result<vk::Pipeline> {
        let vertex_name = if fragment == "floor_light" {
            c"floor_vertex"
        } else {
            c"vertex"
        };
        let vertex_code = shader(vertex_name.to_str()?, naga::ShaderStage::Vertex)?;
        let fragment_code = shader(fragment, naga::ShaderStage::Fragment)?;
        let vertex = unsafe {
            self.device.create_shader_module(
                &vk::ShaderModuleCreateInfo::default().code(&vertex_code),
                None,
            )
        }?;
        let result = (|| -> Result<_> {
            let fragment_module = unsafe {
                self.device.create_shader_module(
                    &vk::ShaderModuleCreateInfo::default().code(&fragment_code),
                    None,
                )
            }?;
            let fragment_name = std::ffi::CString::new(fragment)?;
            let stages = [
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::VERTEX)
                    .module(vertex)
                    .name(vertex_name),
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::FRAGMENT)
                    .module(fragment_module)
                    .name(&fragment_name),
            ];
            let input = vk::PipelineVertexInputStateCreateInfo::default();
            let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport = vk::PipelineViewportStateCreateInfo::default()
                .viewport_count(1)
                .scissor_count(1);
            let raster = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::NONE)
                .line_width(1.0);
            let samples = vk::PipelineMultisampleStateCreateInfo::default()
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);
            let depth = vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(true)
                .depth_write_enable(fragment == "window")
                .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);
            let additive = fragment == "floor_light";
            let attachments = [vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::ONE)
                .dst_color_blend_factor(if additive {
                    vk::BlendFactor::ONE
                } else {
                    vk::BlendFactor::ONE_MINUS_SRC_ALPHA
                })
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .alpha_blend_op(vk::BlendOp::ADD)
                .color_write_mask(if additive {
                    vk::ColorComponentFlags::R
                        | vk::ColorComponentFlags::G
                        | vk::ColorComponentFlags::B
                } else {
                    vk::ColorComponentFlags::RGBA
                })];
            let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
            let dynamic = vk::PipelineDynamicStateCreateInfo::default()
                .dynamic_states(&[vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR]);
            let info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&input)
                .input_assembly_state(&assembly)
                .viewport_state(&viewport)
                .rasterization_state(&raster)
                .multisample_state(&samples)
                .depth_stencil_state(&depth)
                .color_blend_state(&blend)
                .dynamic_state(&dynamic)
                .layout(self.layout)
                .render_pass(self.render_pass);
            let result = unsafe {
                self.device
                    .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
            };
            unsafe {
                self.device.destroy_shader_module(fragment_module, None);
            }
            match result {
                Ok(pipelines) => Ok(pipelines[0]),
                Err((pipelines, error)) => {
                    for pipeline in pipelines {
                        unsafe {
                            self.device.destroy_pipeline(pipeline, None);
                        }
                    }
                    Err(error.into())
                }
            }
        })();
        unsafe {
            self.device.destroy_shader_module(vertex, None);
        }
        result.context("create Vulkan scene pipeline")
    }

    pub unsafe fn draw<'a>(
        &self,
        command: vk::CommandBuffer,
        target: &RenderTarget,
        view: &xr::View,
        panels: impl Iterator<Item = (&'a PanelTexture, PanelGeometry)>,
        cursor: Option<PanelPose>,
        floor_y: f32,
    ) {
        let projection = view_projection(view);
        let mut panels = panels.collect::<Vec<_>>();
        panels.sort_by(|(_, first), (_, second)| {
            let first = (projection * first.pose.center.extend(1.0)).w;
            let second = (projection * second.pose.center.extend(1.0)).w;
            second.total_cmp(&first)
        });
        let area = vk::Rect2D::default().extent(target.extent);
        let clear = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 1.0],
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];
        unsafe {
            self.device.cmd_begin_render_pass(
                command,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(target.framebuffer)
                    .render_area(area)
                    .clear_values(&clear),
                vk::SubpassContents::INLINE,
            );
            self.device.cmd_set_viewport(
                command,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: target.extent.width as f32,
                    height: target.extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            self.device.cmd_set_scissor(command, 0, &[area]);
            self.device.cmd_bind_pipeline(
                command,
                vk::PipelineBindPoint::GRAPHICS,
                self.floor_light_pipeline,
            );
            let eye = [
                view.pose.position.x,
                view.pose.position.y,
                view.pose.position.z,
                0.0,
            ];
            let eye_bytes = std::slice::from_raw_parts(eye.as_ptr().cast::<u8>(), 16);
            self.device.cmd_push_constants(
                command,
                self.layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                112,
                eye_bytes,
            );
            for (texture, geometry) in &panels {
                let pose = geometry.pose;
                let right = pose.orientation() * Vec3::X;
                let up = pose.orientation() * Vec3::Y;
                let height =
                    pose.width_m * geometry.logical_size.h as f32 / geometry.logical_size.w as f32;
                let emitter = [
                    pose.center.x,
                    pose.center.y,
                    pose.center.z,
                    pose.width_m,
                    right.x,
                    right.y,
                    right.z,
                    height,
                    up.x,
                    up.y,
                    up.z,
                    floor_y,
                ];
                let bytes = std::slice::from_raw_parts(emitter.as_ptr().cast::<u8>(), 48);
                self.device.cmd_push_constants(
                    command,
                    self.layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    64,
                    bytes,
                );
                self.device.cmd_bind_descriptor_sets(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.layout,
                    0,
                    &[texture.descriptor],
                    &[],
                );
                self.draw_panel(command, projection);
            }
            self.device.cmd_bind_pipeline(
                command,
                vk::PipelineBindPoint::GRAPHICS,
                self.window_pipeline,
            );
            for (texture, geometry) in panels {
                self.device.cmd_bind_descriptor_sets(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.layout,
                    0,
                    &[texture.descriptor],
                    &[],
                );
                self.draw_panel(command, projection * model(geometry));
            }
            if let Some(mut pose) = cursor {
                pose.center += pose.orientation() * Vec3::Z * 0.001;
                self.device.cmd_bind_pipeline(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.cursor_pipeline,
                );
                self.draw_panel(
                    command,
                    projection
                        * model(PanelGeometry {
                            pose,
                            logical_size: (21, 21).into(),
                        }),
                );
            }
            self.device.cmd_end_render_pass(command);
        }
    }

    unsafe fn draw_panel(&self, command: vk::CommandBuffer, matrix: Mat4) {
        let columns = matrix.to_cols_array();
        let bytes = unsafe { std::slice::from_raw_parts(columns.as_ptr().cast::<u8>(), 64) };
        unsafe {
            self.device.cmd_push_constants(
                command,
                self.layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                bytes,
            );
            self.device.cmd_draw(command, 6, 1, 0, 0);
        }
    }
}

impl Drop for SceneRenderer {
    fn drop(&mut self) {
        unsafe {
            self.device
                .destroy_pipeline(self.floor_light_pipeline, None);
            self.device.destroy_pipeline(self.cursor_pipeline, None);
            self.device.destroy_pipeline(self.window_pipeline, None);
            self.device.destroy_pipeline_layout(self.layout, None);
            self.device.destroy_sampler(self.sampler, None);
            self.device
                .destroy_descriptor_set_layout(self.descriptor_layout, None);
            self.device.destroy_render_pass(self.render_pass, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shaders_compile() {
        for (entry, stage) in [
            ("vertex", naga::ShaderStage::Vertex),
            ("window", naga::ShaderStage::Fragment),
            ("cursor", naga::ShaderStage::Fragment),
            ("floor_vertex", naga::ShaderStage::Vertex),
            ("floor_light", naga::ShaderStage::Fragment),
        ] {
            assert_eq!(shader(entry, stage).unwrap()[0], 0x0723_0203);
        }
    }

    #[test]
    fn asymmetric_projection_matches_vulkan_coordinates() {
        let view = xr::View {
            pose: xr::Posef::IDENTITY,
            fov: xr::Fovf {
                angle_left: -0.7,
                angle_right: 0.9,
                angle_up: 0.8,
                angle_down: -0.6,
            },
        };
        let projection = view_projection(&view);
        for (point, expected) in [
            (
                Vec3::new(view.fov.angle_left.tan(), 0.0, -1.0),
                (-1.0, None),
            ),
            (
                Vec3::new(view.fov.angle_right.tan(), 0.0, -1.0),
                (1.0, None),
            ),
            (
                Vec3::new(0.0, view.fov.angle_up.tan(), -1.0),
                (0.0, Some(-1.0)),
            ),
            (
                Vec3::new(0.0, view.fov.angle_down.tan(), -1.0),
                (0.0, Some(1.0)),
            ),
        ] {
            let ndc = projection.project_point3(point);
            if let Some(vertical) = expected.1 {
                assert!((ndc.y - vertical).abs() < 1.0e-5);
            } else {
                assert!((ndc.x - expected.0).abs() < 1.0e-5);
            }
        }
        assert!(
            projection
                .project_point3(Vec3::new(0.0, 0.0, -0.05))
                .z
                .abs()
                < 1.0e-5
        );
        assert!((projection.project_point3(Vec3::new(0.0, 0.0, -100.0)).z - 1.0).abs() < 1.0e-5);
    }

    #[test]
    fn eye_pose_and_panel_transform_preserve_top_left_coordinates() {
        let orientation = Quat::from_rotation_y(0.4) * Quat::from_rotation_x(-0.2);
        let position = Vec3::new(0.3, 1.7, 0.1);
        let view = xr::View {
            pose: xr::Posef {
                orientation: xr::Quaternionf {
                    x: orientation.x,
                    y: orientation.y,
                    z: orientation.z,
                    w: orientation.w,
                },
                position: xr::Vector3f {
                    x: position.x,
                    y: position.y,
                    z: position.z,
                },
            },
            fov: xr::Fovf {
                angle_left: -std::f32::consts::FRAC_PI_4,
                angle_right: std::f32::consts::FRAC_PI_4,
                angle_up: std::f32::consts::FRAC_PI_4,
                angle_down: -std::f32::consts::FRAC_PI_4,
            },
        };
        let geometry = PanelGeometry {
            pose: PanelPose {
                center: position + orientation * Vec3::NEG_Z * 2.0,
                yaw: 0.4,
                pitch: -0.2,
                width_m: 2.0,
            },
            logical_size: (200, 100).into(),
        };
        let transform = view_projection(&view) * model(geometry);
        let center = transform.project_point3(Vec3::ZERO);
        assert!(center.x.abs() < 1.0e-5 && center.y.abs() < 1.0e-5);
        let top_left = transform.project_point3(Vec3::new(-0.5, 0.5, 0.0));
        assert!((top_left.x + 0.5).abs() < 1.0e-5);
        assert!((top_left.y + 0.25).abs() < 1.0e-5);
    }
}
