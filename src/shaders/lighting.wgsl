const PI: f32 = 3.14159265;

fn fresnel_schlick(cosine: f32) -> f32 {
    let grazing = pow(1.0 - clamp(cosine, 0.0, 1.0), 5.0);
    let reflectance = floor_material.controls.x;
    return reflectance + (1.0 - reflectance) * grazing;
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
