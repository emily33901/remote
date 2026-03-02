@group(group_size_x) @binding(0,0)

@compute
fn i420_to_nv12(
    @texture_2d<rg32> input_y: binding(0),
    @texture_2d<rg32> input_u: binding(1),
    @texture_2d<rg32> input_v: binding(2),
    @texture_2d<rg32, access write> output_y: binding(3),
    @texture_2d<rg32, access write> output_uv: binding(4),
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
    
    if (idx.x < width / 2 && idx.y < height / 2) {
        let u_val = textureLoad(input_u, vec3<u32>(idx.x, idx.y, 0)).r;
        let v_val = textureLoad(input_v, vec3<u32>(idx.x, idx.y, 0)).r;
        textureStore(output_uv, vec3<u32>(idx.x, idx.y, 0), vec4<f32>(u_val, v_val, 0.0, 1.0));
    }
}
