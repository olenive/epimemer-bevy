// The discs' and columns' fragment shader. Every node shares one material
// and one of four meshes of one vertex layout, so Bevy draws them in one
// batch; what differs per node is its mesh tag, which packs its colour as
// sRGB bytes and its alpha into four bytes, red lowest. A mesh's vertex
// colours carry its profile along the column in their alpha, which the
// tag's alpha is multiplied by: whole for a disc or a solid column, fading
// toward an end for a smear. The node is unlit and adds its colour times
// its alpha to what is behind it, as the StandardMaterial's additive mode
// does.

#import bevy_pbr::{
    forward_io::VertexOutput,
    mesh_functions::get_tag,
    mesh_view_bindings::view,
}
#import bevy_render::maths::powsafe

#ifdef TONEMAP_IN_SHADER
#import bevy_core_pipeline::tonemapping::{tone_mapping, screen_space_dither}
#endif

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + vec3(0.055)) / 1.055, vec3(2.4));
    return select(high, low, c <= vec3(0.04045));
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let packed = unpack4x8unorm(get_tag(in.instance_index));
    var colour = vec4(srgb_to_linear(packed.rgb), packed.a);
#ifdef VERTEX_COLORS
    colour.a = colour.a * in.color.a;
#endif

    // Tone mapping and dithering as the StandardMaterial applies them, so
    // the colours read as they did before the discs had their own shader.
#ifdef TONEMAP_IN_SHADER
    colour = tone_mapping(colour, view.color_grading);
#ifdef DEBAND_DITHER
    var rgb = powsafe(colour.rgb, 1.0 / 2.2);
    rgb += screen_space_dither(in.position.xy);
    colour = vec4(powsafe(rgb, 2.2), colour.a);
#endif
#endif

    // Additive blending: the pipeline adds the source to the destination
    // and keeps the destination whole, so the colour is weighted by the
    // alpha here and the alpha written is zero.
    return vec4(colour.rgb * colour.a, 0.0);
}
