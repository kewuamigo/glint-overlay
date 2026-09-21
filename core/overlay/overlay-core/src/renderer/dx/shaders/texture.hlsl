struct vs_out
{
	float4 position : SV_POSITION;
	float2 texCoord : TEXCOORD;
};

cbuffer OverlayBuffer : register(b0)
{
	float4 rect;
}

vs_out vs_main(uint index: SV_VertexID)
{
	static const float2 VERTICES[4] = {
        float2(0.0, 1.0),
        float2(0.0, 0.0),
        float2(1.0, 1.0),
        float2(1.0, 0.0)
	};

	vs_out output;
	float2 pos = VERTICES[index];
	output.position = float4(rect.xy + rect.zw * pos, 0.0, 1.0);
	output.texCoord = pos;

	return output;
}

Texture2D overlay : register(t0);
SamplerState overlaySampler: register(s0);

float4 ps_main(vs_out input) : SV_TARGET
{
	return overlay.Sample(overlaySampler, input.texCoord);
}

// Own HLSL — not Steam CD3D11HDRtoSDR / unk_180119500 / unk_180118E00.
// Mailbox is 8-bit sRGB UNORM. HDR backbuffer needs conversion, not a second present.

float3 srgb_to_linear(float3 c)
{
	return lerp(c / 12.92, pow(abs((c + 0.055) / 1.055), 2.4), step(0.04045, c));
}

float4 ps_scrgb(vs_out input) : SV_TARGET
{
	float4 c = overlay.Sample(overlaySampler, input.texCoord);
	return float4(srgb_to_linear(c.rgb), c.a);
}

float3 rec709_to_rec2020(float3 c)
{
	return mul(float3x3(
		0.6274040, 0.3292820, 0.0433136,
		0.0690970, 0.9195400, 0.0113610,
		0.0163916, 0.0880132, 0.8955950), c);
}

float3 linear_to_pq(float3 lin)
{
	const float m1 = 2610.0 / 16384.0;
	const float m2 = 2523.0 / 32.0;
	const float c1 = 3424.0 / 4096.0;
	const float c2 = 2413.0 / 128.0;
	const float c3 = 2392.0 / 128.0;
	float3 y = pow(saturate(lin), m1);
	return pow((c1 + c2 * y) / (1.0 + c3 * y), m2);
}

float4 ps_pq(vs_out input) : SV_TARGET
{
	float4 c = overlay.Sample(overlaySampler, input.texCoord);
	float3 nits = rec709_to_rec2020(srgb_to_linear(c.rgb)) * (80.0 / 10000.0);
	return float4(linear_to_pq(nits), c.a);
}
