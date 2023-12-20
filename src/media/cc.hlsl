struct VS_Input {
    float2 pos : POS;
    float2 uv : TEX;
};

struct VS_Output {
    float4 pos : SV_POSITION;
    float2 uv : TEXCOORD;
};

VS_Output vs_main(VS_Input input)
{
    VS_Output output;
    output.pos = float4(input.pos, 0.0f, 1.0f);
    output.uv = input.uv;
    return output;
}

Texture2D<float4> inputTexture : register(t0);

RWTexture2D<float> outputTextureY : register(u0);
RWTexture2D<float2> outputTextureUV : register(u1);

SamplerState defaultSampler : register(s0);

void main(VS_Output input)
{
    // Sample input BGRA color
    float4 bgraColor = inputTexture.Sample(defaultSampler, input.uv);

    // Convert to luminance (Y)
    float luminance = 0.299 * bgraColor.r + 0.587 * bgraColor.g + 0.114 * bgraColor.b;

    // Store luminance value in the Y channel
    outputTextureY[input.uv] = luminance;

    // Calculate UV coordinates for chrominance channels
    float2 uvCoord = input.uv * 0.5;

    // Average the two chrominance channels (U and V) from the original BGRA texture
    float2 chrominance = 0.5 * (bgraColor.ba + bgraColor.ga);

    // Store chrominance values in the UV channel
    outputTextureUV[input.uv] = chrominance;
}
