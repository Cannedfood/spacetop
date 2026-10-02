//! OpenXR application client. This connects to the configured system runtime;
//! it does not implement an OpenXR runtime or provide inter-application overlay
//! semantics.

use std::{thread, time::Duration};

use crate::bridge::{PanelUpdate, XrInput};
use crate::panel::{PanelFrame, PanelGeometry, Ray3};
use anyhow::{Context, Result, ensure};
use ash::{
    Entry as VkEntry,
    vk::{self, Handle},
};
use openxr as xr;

type PanelImages = std::collections::HashMap<u64, (PanelFrame, crate::panel::PanelPose)>;

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;
const PANEL_WIDTH: u32 = 512;
const PANEL_HEIGHT: u32 = 512;

fn panel_swapchain_format(formats: &[u32]) -> Result<vk::Format> {
    [vk::Format::R8G8B8A8_SRGB, vk::Format::R8G8B8A8_UNORM]
        .into_iter()
        .find(|format| formats.contains(&(format.as_raw() as u32)))
        .context("runtime lacks RGBA8 swapchain format")
}

fn linearize_panel_colors(frame: &mut PanelFrame) {
    for pixel in frame.bytes.as_chunks_mut::<4>().0 {
        for channel in &mut pixel[..3] {
            let srgb = *channel as f32 / 255.0;
            let linear = if srgb <= 0.04045 {
                srgb / 12.92
            } else {
                ((srgb + 0.055) / 1.055).powf(2.4)
            };
            *channel = (linear * 255.0).round() as u8;
        }
    }
}

/// Run the OpenXR client loop until the runtime requests session/application exit.
///
/// It submits captured Wayland panel pixels as independent quad layers and
/// forwards the right-hand aim/select controller actions to the compositor.
pub fn run(
    frames: calloop::channel::Channel<PanelUpdate>,
    input: calloop::channel::SyncSender<XrInput>,
) -> Result<()> {
    let xr_entry = unsafe { xr::Entry::load(&()) }.context("load OpenXR loader")?;
    let available = xr_entry
        .enumerate_extensions()
        .context("enumerate OpenXR extensions")?;
    ensure!(
        available.khr_vulkan_enable2,
        "configured runtime does not support XR_KHR_vulkan_enable2"
    );

    let mut extensions = xr::ExtensionSet::default();
    extensions.khr_vulkan_enable2 = true;
    let instance = xr_entry
        .create_instance(
            &xr::ApplicationInfo {
                application_name: "spacetop",
                application_version: 1,
                engine_name: "spacetop",
                engine_version: 1,
                api_version: xr::Version::new(1, 0, 0),
            },
            &extensions,
            &[],
            &(),
        )
        .context("create OpenXR instance")?;
    let system = instance
        .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
        .context("find HMD system")?;
    ensure!(
        instance
            .enumerate_view_configurations(system)?
            .contains(&VIEW_TYPE),
        "runtime does not support stereo views"
    );
    let blend_modes = instance.enumerate_environment_blend_modes(system, VIEW_TYPE)?;
    ensure!(
        blend_modes.contains(&xr::EnvironmentBlendMode::OPAQUE),
        "runtime does not support opaque blending"
    );

    let requirements = instance.graphics_requirements::<xr::Vulkan>(system)?;
    let vk_api_version = vk::make_api_version(
        0,
        requirements.min_api_version_supported.major() as u32,
        requirements.min_api_version_supported.minor() as u32,
        requirements.min_api_version_supported.patch(),
    );
    let vk_entry = unsafe { VkEntry::load() }.context("load Vulkan loader")?;
    let vk_app_info = vk::ApplicationInfo::default().api_version(vk_api_version);
    let vk_instance_info = vk::InstanceCreateInfo::default().application_info(&vk_app_info);

    // OpenXR creates the Vulkan instance/device with runtime-required extensions.
    #[allow(clippy::missing_transmute_annotations)]
    let raw_instance = unsafe {
        instance.create_vulkan_instance(
            system,
            std::mem::transmute(vk_entry.static_fn().get_instance_proc_addr),
            &vk_instance_info as *const _ as *const _,
        )
    }
    .context("runtime Vulkan instance creation")?
    .map_err(|error| anyhow::anyhow!("Vulkan instance creation: {error:?}"))?;
    let vk_instance = unsafe {
        ash::Instance::load(
            vk_entry.static_fn(),
            vk::Instance::from_raw(raw_instance as _),
        )
    };
    let raw_physical_device =
        unsafe { instance.vulkan_graphics_device(system, vk_instance.handle().as_raw() as _) }
            .context("query runtime Vulkan device")?;
    let physical_device = vk::PhysicalDevice::from_raw(raw_physical_device as _);
    let queue_family =
        unsafe { vk_instance.get_physical_device_queue_family_properties(physical_device) }
            .iter()
            .position(|properties| properties.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .context("runtime-selected GPU has no graphics queue")? as u32;
    let queue_priorities = [1.0_f32];
    let queue_info = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(queue_family)
        .queue_priorities(&queue_priorities)];
    let device_info = vk::DeviceCreateInfo::default().queue_create_infos(&queue_info);
    #[allow(clippy::missing_transmute_annotations)]
    let raw_device = unsafe {
        instance.create_vulkan_device(
            system,
            std::mem::transmute(vk_entry.static_fn().get_instance_proc_addr),
            physical_device.as_raw() as _,
            &device_info as *const _ as *const _,
        )
    }
    .context("runtime Vulkan device creation")?
    .map_err(|error| anyhow::anyhow!("Vulkan device creation: {error:?}"))?;
    let device =
        unsafe { ash::Device::load(vk_instance.fp_v1_0(), vk::Device::from_raw(raw_device as _)) };
    let queue = unsafe { device.get_device_queue(queue_family, 0) };
    let command_pool = unsafe {
        device.create_command_pool(
            &vk::CommandPoolCreateInfo::default()
                .queue_family_index(queue_family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
            None,
        )
    }
    .context("create Vulkan command pool")?;
    let command_buffers = unsafe {
        device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )
    }
    .context("allocate Vulkan command buffer")?;
    let command_buffer = command_buffers[0];
    let fence = unsafe {
        device.create_fence(
            &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
            None,
        )
    }
    .context("create Vulkan fence")?;

    let xr_session = unsafe {
        instance.create_session::<xr::Vulkan>(
            system,
            &xr::vulkan::SessionCreateInfo {
                instance: vk_instance.handle().as_raw() as _,
                physical_device: physical_device.as_raw() as _,
                device: device.handle().as_raw() as _,
                queue_family_index: queue_family,
                queue_index: 0,
            },
        )
    }
    .context("create OpenXR session")?;
    let (session, mut frame_waiter, mut frame_stream) = xr_session;

    let formats = session.enumerate_swapchain_formats()?;
    let format = panel_swapchain_format(&formats)?;
    let mut swapchain = session.create_swapchain(&xr::SwapchainCreateInfo {
        create_flags: xr::SwapchainCreateFlags::EMPTY,
        usage_flags: xr::SwapchainUsageFlags::TRANSFER_DST,
        format: format.as_raw() as _,
        sample_count: 1,
        width: PANEL_WIDTH,
        height: PANEL_HEIGHT,
        face_count: 1,
        array_size: 1,
        mip_count: 1,
    })?;
    let images = swapchain.enumerate_images()?;
    let space =
        session.create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)?;
    let action_set = instance.create_action_set("spacetop", "Spacetop input", 0)?;
    let right_hand = instance.string_to_path("/user/hand/right")?;
    let aim_action =
        action_set.create_action::<xr::Posef>("aim_pose", "Aim pose", &[right_hand])?;
    let trigger_action = action_set.create_action::<bool>("trigger", "Trigger", &[right_hand])?;
    let aim_path = instance.string_to_path("/user/hand/right/input/aim/pose")?;
    for (profile, button) in [
        (
            "/interaction_profiles/khr/simple_controller",
            "select/click",
        ),
        (
            "/interaction_profiles/oculus/touch_controller",
            "trigger/value",
        ),
        (
            "/interaction_profiles/valve/index_controller",
            "trigger/click",
        ),
        ("/interaction_profiles/htc/vive_controller", "trigger/click"),
        (
            "/interaction_profiles/microsoft/motion_controller",
            "trigger/value",
        ),
    ] {
        instance.suggest_interaction_profile_bindings(
            instance.string_to_path(profile)?,
            &[
                xr::Binding::new(&aim_action, aim_path),
                xr::Binding::new(
                    &trigger_action,
                    instance.string_to_path(&format!("/user/hand/right/input/{button}"))?,
                ),
            ],
        )?;
    }
    session.attach_action_sets(&[&action_set])?;
    let aim_space = aim_action.create_space(&session, right_hand, xr::Posef::IDENTITY)?;
    let mut events = xr::EventDataBuffer::new();
    let mut running = false;
    let mut exit = false;
    let mut panel_frames = PanelImages::new();
    let mut cursor_ray: Option<Ray3> = None;

    while !exit {
        while let Some(event) = instance.poll_event(&mut events)? {
            match event {
                xr::Event::SessionStateChanged(changed)
                    if changed.session() == session.as_raw() =>
                {
                    match changed.state() {
                        xr::SessionState::READY => {
                            session.begin(VIEW_TYPE)?;
                            running = true;
                        }
                        xr::SessionState::STOPPING => {
                            running = false;
                            session.end()?;
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => exit = true,
                        _ => {}
                    }
                }
                xr::Event::InstanceLossPending(_) => exit = true,
                _ => {}
            }
        }
        // Drain compositor-to-XR panel snapshots without blocking the XR frame loop.
        while let Ok(command) = frames.try_recv() {
            match command {
                PanelUpdate::Frame {
                    panel_id,
                    frame,
                    pose,
                } => {
                    panel_frames.insert(panel_id, (frame, pose));
                }
                PanelUpdate::Removed { panel_id } => {
                    panel_frames.remove(&panel_id);
                }
            }
        }
        if exit {
            break;
        }
        if !running {
            thread::sleep(Duration::from_millis(50));
            continue;
        }

        let frame_state = frame_waiter.wait()?;
        // Update the action set and forward the right controller's aim ray.
        session.sync_actions(&[xr::ActiveActionSet::new(&action_set)])?;
        if aim_action.is_active(&session, right_hand)? {
            let controller = aim_space.locate(&space, frame_state.predicted_display_time)?;
            if controller
                .location_flags
                .contains(xr::SpaceLocationFlags::POSITION_VALID)
                && controller
                    .location_flags
                    .contains(xr::SpaceLocationFlags::ORIENTATION_VALID)
            {
                let pose = controller.pose;
                let orientation = glam::Quat::from_xyzw(
                    pose.orientation.x,
                    pose.orientation.y,
                    pose.orientation.z,
                    pose.orientation.w,
                );
                let direction = orientation * glam::Vec3::NEG_Z;
                cursor_ray = Some(Ray3 {
                    origin: glam::Vec3::new(pose.position.x, pose.position.y, pose.position.z),
                    direction,
                });
                let time_ms = (frame_state.predicted_display_time.as_nanos() / 1_000_000) as u32;
                let _ = input.try_send(XrInput::Ray {
                    ray: cursor_ray.expect("ray assigned above"),
                    time_ms,
                });
                let trigger = trigger_action.state(&session, right_hand)?;
                if trigger.changed_since_last_sync {
                    let _ = input.try_send(XrInput::Button {
                        pressed: trigger.current_state,
                        time_ms,
                    });
                }
            }
        } else {
            cursor_ray = None;
        }
        frame_stream.begin()?;
        if !frame_state.should_render {
            frame_stream.end(
                frame_state.predicted_display_time,
                xr::EnvironmentBlendMode::OPAQUE,
                &[],
            )?;
            continue;
        }

        let image_index = swapchain.acquire_image()?;
        swapchain.wait_image(xr::Duration::INFINITE)?;
        let image = vk::Image::from_raw(
            *images
                .get(image_index as usize)
                .context("bad swapchain index")? as _,
        );
        unsafe {
            device.wait_for_fences(&[fence], true, u64::MAX)?;
            device.reset_fences(&[fence])?;
            device.reset_command_buffer(command_buffer, vk::CommandBufferResetFlags::empty())?;
            device.begin_command_buffer(
                command_buffer,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(0)
                .level_count(1)
                .base_array_layer(0)
                .layer_count(1);
            let begin_barrier = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(range);
            let panel = panel_frames
                .iter()
                .min_by_key(|(id, _)| *id)
                .map(|(_, (frame, pose))| (frame, pose));
            let panel_blit = if let Some((source_frame, pose)) = panel {
                let mut upload_frame = source_frame.clone();
                if let Some(ray) = cursor_ray {
                    let geometry = PanelGeometry {
                        pose: *pose,
                        logical_size: smithay::utils::Size::from((
                            source_frame.size.w,
                            source_frame.size.h,
                        )),
                    };
                    if let Some(hit) = geometry.intersect(ray) {
                        let (width, height) =
                            (upload_frame.size.w as usize, upload_frame.size.h as usize);
                        let center_x = hit.surface_px.x.round() as i32;
                        let center_y = hit.surface_px.y.round() as i32;
                        for offset in -10_i32..=10 {
                            for (x, y) in
                                [(center_x + offset, center_y), (center_x, center_y + offset)]
                            {
                                if x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < height
                                {
                                    let at = y as usize * upload_frame.stride + x as usize * 4;
                                    upload_frame.bytes[at..at + 4]
                                        .copy_from_slice(&[255, 245, 0, 255]);
                                }
                            }
                        }
                    }
                }
                if format == vk::Format::R8G8B8A8_UNORM {
                    linearize_panel_colors(&mut upload_frame);
                }
                let frame = &upload_frame;
                let width = frame.size.w as u32;
                let height = frame.size.h as u32;
                let byte_len = frame
                    .stride
                    .checked_mul(height as usize)
                    .context("panel staging-buffer size overflow")?;
                ensure!(
                    frame.bytes.len() >= byte_len,
                    "panel snapshot is shorter than its stride/height"
                );
                let staging_info = vk::BufferCreateInfo::default()
                    .size(byte_len as u64)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE);
                let staging = device.create_buffer(&staging_info, None)?;
                let requirements = device.get_buffer_memory_requirements(staging);
                let memory_properties =
                    vk_instance.get_physical_device_memory_properties(physical_device);
                let memory_type = (0..memory_properties.memory_type_count)
                    .find(|index| {
                        let type_supported = requirements.memory_type_bits & (1 << index) != 0;
                        let flags = memory_properties.memory_types[*index as usize].property_flags;
                        type_supported
                            && flags.contains(
                                vk::MemoryPropertyFlags::HOST_VISIBLE
                                    | vk::MemoryPropertyFlags::HOST_COHERENT,
                            )
                    })
                    .context("no host-coherent Vulkan memory for panel upload")?;
                let staging_memory = device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(requirements.size)
                        .memory_type_index(memory_type),
                    None,
                )?;
                device.bind_buffer_memory(staging, staging_memory, 0)?;
                let mapped = device.map_memory(
                    staging_memory,
                    0,
                    byte_len as u64,
                    vk::MemoryMapFlags::empty(),
                )?;
                std::ptr::copy_nonoverlapping(frame.bytes.as_ptr(), mapped.cast::<u8>(), byte_len);
                device.unmap_memory(staging_memory);
                Some((
                    staging,
                    staging_memory,
                    width,
                    height,
                    (frame.stride / 4) as u32,
                ))
            } else {
                None
            };
            device.cmd_pipeline_barrier(
                command_buffer,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[begin_barrier],
            );
            device.cmd_clear_color_image(
                command_buffer,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &vk::ClearColorValue {
                    float32: [0.08, 0.12, 0.20, 1.0],
                },
                &[range],
            );
            if let Some((staging, _, width, height, row_length)) = panel_blit {
                device.cmd_copy_buffer_to_image(
                    command_buffer,
                    staging,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[vk::BufferImageCopy::default()
                        .buffer_row_length(row_length)
                        .buffer_image_height(height)
                        .image_subresource(vk::ImageSubresourceLayers {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            mip_level: 0,
                            base_array_layer: 0,
                            layer_count: 1,
                        })
                        .image_extent(vk::Extent3D {
                            width,
                            height,
                            depth: 1,
                        })],
                );
            }
            let end_barrier = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::MEMORY_READ)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(range);
            device.cmd_pipeline_barrier(
                command_buffer,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[end_barrier],
            );
            device.end_command_buffer(command_buffer)?;
            device.queue_submit(
                queue,
                &[vk::SubmitInfo::default().command_buffers(&[command_buffer])],
                fence,
            )?;
            device.wait_for_fences(&[fence], true, u64::MAX)?;
            if let Some((staging, staging_memory, _, _, _)) = panel_blit {
                device.destroy_buffer(staging, None);
                device.free_memory(staging_memory, None);
            }
        }
        swapchain.release_image()?;

        let panels = panel_frames
            .iter()
            // Until each panel gets its own swapchain, the Vulkan upload target
            // can display only one independent window image.
            .take(1)
            .map(|(id, (frame, pose))| (*id, frame, *pose))
            .collect::<Vec<_>>();
        let quads = panels
            .iter()
            .map(|(_, frame, pose)| {
                let orientation = glam::Quat::from_rotation_y(pose.yaw);
                let size = xr::Rect2Di {
                    offset: xr::Offset2Di { x: 0, y: 0 },
                    extent: xr::Extent2Di {
                        width: frame.size.w,
                        height: frame.size.h,
                    },
                };
                xr::CompositionLayerQuad::new()
                    .space(&space)
                    .layer_flags(xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA)
                    .sub_image(
                        xr::SwapchainSubImage::new()
                            .swapchain(&swapchain)
                            .image_array_index(0)
                            .image_rect(size),
                    )
                    .pose(xr::Posef {
                        orientation: xr::Quaternionf {
                            x: orientation.x,
                            y: orientation.y,
                            z: orientation.z,
                            w: orientation.w,
                        },
                        position: xr::Vector3f {
                            x: pose.center.x,
                            y: pose.center.y,
                            z: pose.center.z,
                        },
                    })
                    .size(xr::Extent2Df {
                        width: pose.width_m,
                        height: pose.width_m * frame.size.h as f32 / frame.size.w.max(1) as f32,
                    })
            })
            .collect::<Vec<_>>();
        let layers = quads
            .iter()
            .map(|quad| quad as &xr::CompositionLayerBase<xr::Vulkan>)
            .collect::<Vec<_>>();
        frame_stream.end(
            frame_state.predicted_display_time,
            xr::EnvironmentBlendMode::OPAQUE,
            &layers,
        )?;

        // Input actions/controller poses are not yet created; `input` is the
        // channel used by the later tracked-ray/action integration.
        let _ = &input;
    }

    unsafe {
        device.device_wait_idle()?;
        device.destroy_fence(fence, None);
        device.destroy_command_pool(command_pool, None);
        device.destroy_device(None);
        vk_instance.destroy_instance(None);
    }
    Ok(())
}

#[cfg(test)]
mod color_tests {
    use super::*;

    #[test]
    fn prefers_srgb_and_supports_linear_fallback() {
        let srgb = vk::Format::R8G8B8A8_SRGB;
        let linear = vk::Format::R8G8B8A8_UNORM;
        assert_eq!(
            panel_swapchain_format(&[linear.as_raw() as u32, srgb.as_raw() as u32])
                .unwrap()
                .as_raw(),
            srgb.as_raw()
        );
        assert_eq!(
            panel_swapchain_format(&[linear.as_raw() as u32])
                .unwrap()
                .as_raw(),
            linear.as_raw()
        );
        assert!(panel_swapchain_format(&[]).is_err());
    }

    #[test]
    fn linear_fallback_corrects_midtones_without_changing_alpha() {
        let mut frame = PanelFrame {
            size: smithay::utils::Size::from((2, 1)),
            stride: 8,
            bytes: vec![0, 128, 255, 255, 10, 64, 192, 127],
        };
        linearize_panel_colors(&mut frame);
        assert_eq!(frame.bytes, [0, 55, 255, 255, 1, 13, 134, 127]);
    }
}
