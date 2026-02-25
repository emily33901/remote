#version 330 core

in vec2 v_uv;

uniform sampler2D y_tex;
uniform sampler2D uv_tex;

out vec4 FragColor;

// BT.601 YUV to RGB conversion
// https://msdn.microsoft.com/en-us/library/windows/desktop/dd206750(v=vs.85).aspx
vec3 yuv_to_rgb(float y, vec2 uv) {
    // Offset for YUV (16 for Y, 128 for UV)
    vec3 yuv = vec3(y - 0.062745, uv.r - 0.501960, uv.g - 0.501960);
    
    // YUV to RGB conversion matrix (BT.601)
    mat3 yuv_to_rgb_matrix = mat3(
        1.164383,  1.164383,  1.164383,
        0.000000, -0.391762,  2.017232,
        1.596027, -0.812968,  0.000000
    );
    
    return clamp(yuv * yuv_to_rgb_matrix, 0.0, 1.0);
}

// RGB to sRGB
vec3 rgb_to_srgb(vec3 rgb) {
    vec3 s1 = sqrt(rgb);
    vec3 s2 = sqrt(s1);
    vec3 s3 = sqrt(s2);
    return clamp(0.585122381 * s1 + 0.783140355 * s2 - 0.368262736 * s3, 0.0, 1.0);
}

// sRGB to RGB
vec3 srgb_to_rgb(vec3 srgb) {
    return 0.012522878 * srgb +
        0.682171111 * srgb * srgb +
        0.305306011 * srgb * srgb * srgb;
}

void main() {
    float y = texture(y_tex, v_uv).r;
    vec2 uv = texture(uv_tex, v_uv).rg;
    
    vec3 rgb = yuv_to_rgb(y, uv);
    vec3 srgb = rgb_to_srgb(rgb);
    vec3 final_rgb = srgb_to_rgb(srgb);
    
    FragColor = vec4(final_rgb, 1.0);
}
