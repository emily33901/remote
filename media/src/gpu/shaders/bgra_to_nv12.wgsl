@group(group_size_x) @binding(0,0)

@compute
fn bgra_to_nv12(
    @texture_2d<f32> input: binding(0),
    @texture_2d<rg32, access write> output_y: binding(1),
    @texture_2d<rg32, access write> output_uv: binding(2),
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(num_workgroups) num_workgroups: vec3<u32>,
    @builtin(workgroup_size) workgroup_size: vec3<u32>
) {
    let pixel_idx = global_id * workgroup_size + vec3<u32>(
            gl_LocalInvocationID.x,
            gl_LocalInvocationID.y,
            gl_LocalInvocationID.z
        );
    
    let width = textureDimensions(input).x;
    let height = textureDimensions(input).y;
    
    if (pixel_idx.x >= width || pixel_idx.y >= height) {
        return;
    }
    
    let bgra = textureLoad(input, pixel_idx);
    
    let b = bgra.r;
    let g = bgra.g;
    let r = bgra.b;
    
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    
    textureStore(output_y, pixel_idx, vec4<f32>(y, y, y, 1.0));
    
    let uv_x = pixel_idx.x / 2;
    let uv_y = pixel_idx.y / 2;
    
    if (uv_x < width / 2 && uv_y < height / 2) {
        let uv_idx = vec3<u32>(uv_x, uv_y, 0);
        
        let u = textureLoad(input, vec3<u32>(pixel_idx.x * 2, pixel_idx.y * 2, 0)).r;
        let v = textureLoad(input, vec3<u32>(pixel_idx.x * 2 + 1, pixel_idx.y * 2, 0)).r;
        
        textureStore(output_uv, uv_idx, vec4<f32>(u, v, 0.0, 1.0));
    }
}
