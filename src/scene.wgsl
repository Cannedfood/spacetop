const PI: f32 = 3.14159265;

struct Transform {
    matrix: mat4x4<f32>,
    emitter_center_width: vec4<f32>,
    emitter_right_height: vec4<f32>,
    emitter_up: vec4<f32>,
    window_info: vec4<f32>,
    eye_position: vec4<f32>,
}
var<immediate> transform: Transform;
@group(0) @binding(0) var panel: texture_2d<f32>;
@group(0) @binding(1) var filtering: sampler;
struct FloorMaterial {
    albedo: vec4<f32>,
    controls: vec4<f32>,
    sampling: vec4<f32>,
}
@group(1) @binding(0) var<uniform> floor_material: FloorMaterial;
struct SkyVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) direction: vec3<f32>,
}
@vertex fn sky_vertex(@builtin(vertex_index) index: u32) -> SkyVertex {
    let corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 0.0),
        vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 1.0));
    let uv = corners[index];
    var result: SkyVertex;
    result.position = vec4(uv * 2.0 - 1.0, 1.0, 1.0);
    result.uv = uv;
    result.direction = (transform.matrix * vec4(uv, 0.0, 1.0)).xyz;
    return result;
}
fn fresnel_schlick(cosine: f32) -> f32 {
    let grazing = pow(1.0 - clamp(cosine, 0.0, 1.0), 5.0);
    let reflectance = floor_material.controls.x;
    return reflectance + (1.0 - reflectance) * grazing;
}
fn skybox_exposure() -> f32 {
    return exp2(floor_material.controls.y);
}
fn skybox_uv(direction: vec3<f32>) -> vec2<f32> {
    let longitude = atan2(direction.z, direction.x) + floor_material.sampling.y;
    return vec2(
        fract(longitude / (2.0 * PI) + 0.5),
        0.5 - asin(clamp(direction.y, -1.0, 1.0)) / PI);
}
fn sample_skybox(direction: vec3<f32>, roughness: f32) -> vec3<f32> {
    let uv = skybox_uv(direction);
    var hdr = textureSampleLevel(panel, filtering, uv, 0.0).rgb;
    let blur = roughness * roughness * 0.04;
    if blur > 0.0 {
        hdr += textureSampleLevel(panel, filtering, uv + vec2(blur, 0.0), 0.0).rgb;
        hdr += textureSampleLevel(panel, filtering, uv - vec2(blur, 0.0), 0.0).rgb;
        hdr += textureSampleLevel(panel, filtering, uv + vec2(0.0, blur), 0.0).rgb;
        hdr += textureSampleLevel(panel, filtering, uv - vec2(0.0, blur), 0.0).rgb;
        hdr *= 0.2;
    }
    hdr = max(hdr * skybox_exposure(), vec3(0.0));
    return hdr / (vec3(1.0) + hdr);
}
@fragment fn sky(input: SkyVertex) -> @location(0) vec4<f32> {
    let direction = normalize(input.direction);
    let floor_distance = (transform.emitter_up.w - transform.eye_position.y) / direction.y;
    if direction.y < 0.0 && floor_distance > 0.0 {
        let fresnel = fresnel_schlick(-direction.y);
        let albedo = floor_material.albedo.rgb * (1.0 - fresnel);
        let opacity = floor_material.albedo.a;
        var transmitted_background = vec3(0.0);
        if opacity < 1.0 {
            transmitted_background = sample_skybox(direction, 0.0);
        }
        let reflection_direction = reflect(direction, vec3(0.0, 1.0, 0.0));
        let reflected_sky = sample_skybox(reflection_direction, floor_material.controls.z);
        let ground = albedo * opacity
            + transmitted_background * (1.0 - opacity)
            + reflected_sky * fresnel * opacity;
        return vec4(ground, 1.0);
    }
    return vec4(sample_skybox(direction, 0.0), 1.0);
}
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
    let margin = 0.035;
    let edge = min(min(input.uv.x, 1.0 - input.uv.x), min(input.uv.y, 1.0 - input.uv.y));
    if edge < 0.0 { discard; }
    if transform.window_info.w > 0.5 && edge < margin {
        return vec4(1.0, 0.9131, 0.0, 1.0);
    }
    let color = textureSample(panel, filtering, (input.uv - vec2(margin)) / (1.0 - 2.0 * margin));
    if color.a < 0.001 { discard; }
    return color;
}
@fragment fn cursor(input: Vertex) -> @location(0) vec4<f32> {
    let stroke = max(vec2(0.5 / 21.0), fwidth(input.uv) * 0.75);
    if all(abs(input.uv - vec2(0.5)) > stroke) { discard; }
    return vec4(1.0, 0.9131, 0.0, 1.0);
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
    let cell = bitcast<vec2<u32>>(
        vec2<i32>(round(position.xz / floor_material.sampling.x)));
    let seed = sample_hash(cell.x ^ sample_hash(cell.y) ^ sample_hash(index + 0x9e3779b9u));
    let random = vec2(sample_hash(seed), sample_hash(seed ^ 0x85ebca6bu));
    return vec2<f32>(random >> vec2(8u)) / 16777216.0;
}
@fragment fn floor_light(input: SkyVertex) -> @location(0) vec4<f32> {
    let center = transform.emitter_center_width.xyz;
    let width = transform.emitter_center_width.w;
    let right = transform.emitter_right_height.xyz;
    let height = transform.emitter_right_height.w;
    let up = transform.emitter_up.xyz;
    let normal = cross(right, up);
    let eye = transform.eye_position.xyz;
    let incident = normalize(input.direction);
    if incident.y >= 0.0 { return vec4(0.0); }
    let floor_distance = (transform.emitter_up.w - eye.y) / incident.y;
    if floor_distance <= 0.0 { return vec4(0.0); }
    let world = eye + incident * floor_distance;
    let view = -incident;
    let roughness = floor_material.controls.z;
    let ray_count = u32(floor_material.controls.w);
    let alpha = max(0.001, roughness * roughness);
    let alpha_squared = alpha * alpha;
    let view_masking = ggx_masking(view.y, alpha_squared);
    let rows = max(1u, u32(sqrt(f32(ray_count))));
    let short_row_count = ray_count / rows;
    let long_rows = ray_count % rows;
    var sum = vec3(0.0);
    for (var index = 0u; index < ray_count; index += 1u) {
        let row = index % rows;
        let columns = short_row_count + select(0u, 1u, row < long_rows);
        let row_offset = row * short_row_count + min(row, long_rows);
        let jitter = sample_jitter(world, index);
        let sample_uv = vec2(
            (f32(index / rows) + jitter.x) / f32(columns),
            (f32(row_offset) + jitter.y * f32(columns)) / f32(ray_count));
        let half_vector = sample_ggx_visible_normal(view, sample_uv, alpha);
        let ray = reflect(-view, half_vector);
        let denominator = dot(ray, normal);
        if denominator >= -0.00001 || ray.y <= 0.0 { continue; }
        let distance = dot(center - world, normal) / denominator;
        if distance <= 0.0 { continue; }
        let hit = world + ray * distance - center;
        let uv = vec2(dot(hit, right) / width + 0.5, 0.5 - dot(hit, up) / height);
        if any(uv < vec2(0.0)) || any(uv > vec2(1.0)) { continue; }
        let color = textureSampleLevel(panel, filtering, uv, 0.0).rgb;
        let view_half = clamp(dot(view, half_vector), 0.0, 1.0);
        let fresnel = fresnel_schlick(view_half);
        let light_masking = ggx_masking(ray.y, alpha_squared);
        let weight = fresnel * light_masking
            / (view_masking + light_masking - view_masking * light_masking);
        sum += color * weight;
    }
    return vec4(sum / f32(ray_count), 0.0);
}