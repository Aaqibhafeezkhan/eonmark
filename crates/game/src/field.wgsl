// The ground's fragment shader (M3b): the StandardMaterial with territory
// laid over the base colour before lighting, so it stays matte. Owned land
// is tinted toward its owner's team colour and a soft line about one tile
// wide follows every change of ownership (docs/ART_STYLE.md, "Territory and
// fog"). Forward only: the game never enables the deferred prepass.
//
// The field texture holds one texel per sim tile, row-major from the
// north-west: R the owner code (0 neutral, else player index + 1), G the
// owning layer's strength, B fog, A supply. Only R is read today; fog and
// the supply hatching (both M7; M5a owner decision 10 moved the hatching
// there, M5a draws a supply ring instead) will read the other channels.
// `territory` has a line by line CPU copy, `territory_at` in the tests of
// `field.rs`, which pin its look; change both together.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
}

const MAX_TEAMS: u32 = 8u;

struct FieldParams {
    // Linear RGB per player slot; alpha unused.
    team: array<vec4<f32>, 8>,
    // Field width and height in tiles.
    size: vec2<f32>,
    // World XZ of the field's north-west corner.
    origin: vec2<f32>,
    // How far owned land is blended toward its team colour.
    tint: f32,
    // How far the centre of the border line is blended toward it.
    border: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var field_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var<uniform> params: FieldParams;

// Owner code of a tile, clamped onto the field so the map edge is never a
// border.
fn owner_code(tile: vec2<i32>) -> u32 {
    let last = vec2<i32>(params.size) - vec2<i32>(1);
    let texel = textureLoad(field_texture, clamp(tile, vec2<i32>(0), last), 0);
    return u32(round(texel.r * 255.0));
}

fn team_color(code: u32) -> vec3<f32> {
    return params.team[min(code - 1u, MAX_TEAMS - 1u)].rgb;
}

struct Territory {
    // Team colour and coverage (0..1) of the tint.
    tint: vec4<f32>,
    // Colour and strength (0..1) of the border line.
    line: vec4<f32>,
}

// The territory at tile-space point `p` (tile (x, y) covers [x, x+1) x
// [y, y+1)). Each owner's membership is interpolated between the four
// nearest tile centres with smoothstep weights; the isoline where it
// crosses one half is the border, and the line fades to nothing at the
// tile centres on either side.
fn territory(p: vec2<f32>) -> Territory {
    var out: Territory;
    out.tint = vec4<f32>(0.0);
    out.line = vec4<f32>(0.0);
    if any(p < vec2<f32>(0.0)) || any(p >= params.size) {
        return out;
    }
    let q = p - vec2<f32>(0.5);
    let nw = vec2<i32>(floor(q));
    let f = q - floor(q);
    let s = f * f * (3.0 - 2.0 * f);
    var codes = array<u32, 4>(
        owner_code(nw),
        owner_code(nw + vec2<i32>(1, 0)),
        owner_code(nw + vec2<i32>(0, 1)),
        owner_code(nw + vec2<i32>(1, 1)),
    );
    var weights = array<f32, 4>(
        (1.0 - s.x) * (1.0 - s.y),
        s.x * (1.0 - s.y),
        (1.0 - s.x) * s.y,
        s.x * s.y,
    );
    var tint_rgb = vec3<f32>(0.0);
    var tint_cover = 0.0;
    var line_rgb = vec3<f32>(0.0);
    var line_sum = 0.0;
    var line_max = 0.0;
    for (var i = 0u; i < 4u; i++) {
        let code = codes[i];
        if code == 0u {
            continue;
        }
        var first = true;
        for (var j = 0u; j < i; j++) {
            if codes[j] == code {
                first = false;
            }
        }
        if !first {
            continue;
        }
        var m = 0.0;
        for (var j = i; j < 4u; j++) {
            if codes[j] == code {
                m += weights[j];
            }
        }
        let color = team_color(code);
        tint_rgb += m * color;
        tint_cover += m;
        let t = 1.0 - abs(2.0 * m - 1.0);
        let l = t * t * (3.0 - 2.0 * t);
        // Weighted by membership too, so each side of a shared border
        // leans toward its own owner and the edge is an even mix.
        line_rgb += l * m * color;
        line_sum += l * m;
        line_max = max(line_max, l);
    }
    if tint_cover > 0.0 {
        out.tint = vec4<f32>(tint_rgb / tint_cover, tint_cover);
    }
    if line_sum > 0.0 {
        out.line = vec4<f32>(line_rgb / line_sum, line_max);
    }
    return out;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let t = territory(in.world_position.xz - params.origin);
    var rgb = pbr_input.material.base_color.rgb;
    rgb = mix(rgb, t.tint.rgb, params.tint * t.tint.a);
    rgb = mix(rgb, t.line.rgb, params.border * t.line.a);
    pbr_input.material.base_color = vec4<f32>(rgb, pbr_input.material.base_color.a);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
