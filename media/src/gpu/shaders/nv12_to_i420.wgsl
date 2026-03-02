@group(group_size_x) @binding(0,0)

@compute
fn nv12_to_i420(
    @texture_2d<rg32> input_y: binding(0),
    @texture_2d<rg32> input_uv: binding(1),
    @texture_2d<rg32, access write> output_y: binding(2),
    @texture_2d<rg32, access write> output_u: binding(3),
    @texture_2d<rg32, access write> output_v: binding(4),
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(num_workgroups) num_workgroups: vec3<u32>,
    @builtin(workgroup_size) workgroup_size: vec3<u32>
) {
    let idx = global_id * workgroup_size + vec3<u32>(
        gl_LocalInvocationID.x,
        gl_LocalInvocationID.y,
        gl_LocalInvocationID.z
    );
    
    let width = textureDimensions(input_y).x;
    let height = textureDimensions(input_y).y;
    
    if (idx.x >= width || idx.y >= height) {
        return;
    }
    
    let y_val = textureLoad(input_y, vec3<u32>(idx.x, idx.y, 0)).r;
    textureStore(output_y, vec3<u32>(idx.x, idx.y, 0), vec4<f32>(y_val, y_val, y_val, 1.0));
    
    let uv_width = width / 2;
    let uv_height = height / 2;
    
    if (idx.x < uv_width && idx.y < uv_height) {
        let uv = textureLoad(input_uv, vec3<u32>(idx.x, idx.y, 0));
        let u = uv.r;
        let v = uv.g;
        
        textureStore(output_u, vec3<u32>(idx.x, idx.y, 0), vec4<f32>(u, u, u, 1.0));
        textureStore(output_v, vec3<u32>(idx.x, idx.y, 0), vec4<f32>(v, v, v, 1.0));
    }
}
