struct VS_Input {
    float2 pos : POS;
    float2 uv : TEX;
};

struct VS_Output {
    float4 pos : SV_POSITION;
    float2 uv : TEXCOORD;
};

Texture2D<float>  luminanceChannel   : t0;
Texture2D<float2> chrominanceChannel : t1;
SamplerState      defaultSampler     : s0;

VS_Output vs_main(VS_Input input)
{
    VS_Output output;
    output.pos = float4(input.pos, 0.0f, 1.0f);
    output.uv = input.uv;
    return output;
}


// Derived from https://msdn.microsoft.com/en-us/library/windows/desktop/dd206750(v=vs.85).aspx
// Section: Converting 8-bit YUV to RGB888
static const float3x3 YUVtoRGBCoeffMatrix = 
{
    1.164383f,  1.164383f, 1.164383f,
    0.000000f, -0.391762f, 2.017232f,
    1.596027f, -0.812968f, 0.000000f
};

float3 ConvertYUVtoRGB(float3 yuv)
{
    // Derived from https://msdn.microsoft.com/en-us/library/windows/desktop/dd206750(v=vs.85).aspx
    // Section: Converting 8-bit YUV to RGB888

    // These values are calculated from (16 / 255) and (128 / 255)
    yuv -= float3(0.062745f, 0.501960f, 0.501960f);
    yuv = mul(yuv, YUVtoRGBCoeffMatrix);

    return saturate(yuv);
}

float4 ps_main(VS_Output input) : SV_Target
{
    float y = luminanceChannel.Sample(defaultSampler, input.uv);
    float2 uv = chrominanceChannel.Sample(defaultSampler, input.uv);

    return min16float4(ConvertYUVtoRGB(float3(y, uv)), 1.f);
}
