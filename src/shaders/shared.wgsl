enable wgpu_binding_array;

struct Transform {
    matrix: mat4x4<f32>,
    emitter_center_width: vec4<f32>,
    emitter_right_height: vec4<f32>,
    emitter_up: vec4<f32>,
    eye_position: vec4<f32>,
}
struct FloorMaterial {
    albedo: vec4<f32>,
    controls: vec4<f32>,
    sampling: vec4<f32>,
    window_style: vec4<f32>,
    border_color: vec4<f32>,
    cursor_close_border_color: vec4<f32>,
    grabbed_style: vec4<f32>,
    grabbed_border_color: vec4<f32>,
    diffuse_irradiance: vec4<f32>,
}
struct Window {
    center_width: vec4<f32>,
    right_height: vec4<f32>,
    up: vec4<f32>,
    atlas_rect: vec4<f32>,
}
struct WindowBuffer {
    count: u32,
    padding0: u32,
    padding1: u32,
    padding2: u32,
    windows: array<Window>,
}
struct WindowHit {
    distance: f32,
    index: u32,
    uv: vec2<f32>,
    found: bool,
}

var<immediate> transform: Transform;
@group(0) @binding(0) var panel: texture_2d<f32>;
@group(0) @binding(1) var filtering: sampler;
@group(1) @binding(0) var<uniform> floor_material: FloorMaterial;
@group(2) @binding(0) var<storage, read> window_buffer: WindowBuffer;
@group(2) @binding(1) var environment_filter: sampler;
@group(2) @binding(2) var environment_skybox: texture_2d<f32>;
@group(2) @binding(3) var environment_sky_filter: sampler;
@group(2) @binding(4) var panel_textures: binding_array<texture_2d<f32>>;
