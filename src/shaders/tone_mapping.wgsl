fn reinhard_tone_map(hdr: vec3<f32>) -> vec3<f32> {
    return hdr / (vec3(1.0) + hdr);
}
