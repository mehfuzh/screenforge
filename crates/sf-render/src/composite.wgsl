// Single "quad" shader used for every layer. `mode` selects the fragment behaviour.
//   0 solid colour with rounded corners
//   1 textured with rounded corners (screen / camera / background image)
//   2 linear gradient (full canvas)
//   3 soft shadow (rounded-rect SDF blurred by `blur`; `extra.x` > 0 = directional falloff)
//   4 textured sprite (cursor), rotated around `pivot`, no corner rounding
//   5 blurred texture region tinted toward `color` (sensitive-data mask)
//   6 dim `dst` by `color` except inside the `stops` rects (highlight mask)
//   7 disc + ring (click effect): `extra.x` radius, `extra.y` ring width, `color` fill, `stops[0]` edge
//   8 premultiplied texture with rounded corners (overlays, titles, held frames), blurred by
//     `extra.xy` (uv radius) when non-zero

struct Params {
    out_size: vec2<f32>,
    pivot: vec2<f32>,
    dst: vec4<f32>,          // x, y, w, h in output pixels
    uv: vec4<f32>,           // u0, v0, u1, v1
    radius: f32,
    mode: u32,
    blur: f32,
    alpha: f32,
    color: vec4<f32>,
    grad_start: vec2<f32>,   // mode 2: gradient start; mode 3: shadow offset; mode 5: clip uv min
    grad_end: vec2<f32>,     // mode 2: gradient end;                          mode 5: clip uv max
    rotation: f32,           // radians, clockwise, around pivot
    mirror: u32,
    stop_count: u32,
    grade: u32,              // mode 1: 1 = run the camera grade below
    extra: vec4<f32>,        // per-mode scalars (see above)
    stops: array<vec4<f32>, 8>, // mode 2: rgb, position; mode 6: hole rects; mode 7: edge colour
    // camera grade (Resolve style primaries, math units; see sf_render::CameraGrade)
    grade_lift: vec4<f32>,
    grade_gamma: vec4<f32>,  // exponents
    grade_gain: vec4<f32>,
    grade_offset: vec4<f32>,
    grade_a: vec4<f32>,      // temperature, tint, contrast, pivot
    grade_b: vec4<f32>,      // shadows, highlights, color boost, saturation
    grade_c: vec4<f32>,      // hue turns, lut mix, grain, sharpen
    grade_d: vec4<f32>,      // lut size, texel w, texel h, grain seed
};

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var lut: texture_3d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) px: vec2<f32>,      // output pixel coords (unrotated quad space)
};

fn rot(v: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec2<f32>(c * v.x - s * v.y, s * v.x + c * v.y);
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // two triangles covering dst expanded by the blur radius (+ shadow offset)
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0));
    let c = corners[vi];
    var pad = 0.0;
    if (P.mode == 3u) {
        pad = P.blur * 3.0 + max(abs(P.grad_start.x), abs(P.grad_start.y)) + 1.0;
    }
    let origin = P.dst.xy - vec2<f32>(pad, pad);
    let size = P.dst.zw + vec2<f32>(pad * 2.0, pad * 2.0);
    let local = origin + c * size;           // unrotated pixel position
    var world = local;
    if (P.mode == 4u && P.rotation != 0.0) {
        world = P.pivot + rot(local - P.pivot, P.rotation);
    }
    let ndc = vec2<f32>(world.x / P.out_size.x * 2.0 - 1.0, 1.0 - world.y / P.out_size.y * 2.0);
    var o: VsOut;
    o.pos = vec4<f32>(ndc, 0.0, 1.0);
    o.px = local;
    return o;
}

fn sd_round_box(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let rr = min(r, min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(rr, rr);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - rr;
}

fn gradient_color(t_in: f32) -> vec3<f32> {
    let t = clamp(t_in, 0.0, 1.0);
    var col = P.stops[0].rgb;
    for (var i = 1u; i < P.stop_count; i = i + 1u) {
        let a = P.stops[i - 1u];
        let b = P.stops[i];
        let f = clamp((t - a.w) / max(b.w - a.w, 1e-5), 0.0, 1.0);
        if (t >= a.w) {
            col = mix(a.rgb, b.rgb, f);
        }
    }
    return col;
}

// Interleaved gradient noise (Jimenez), used to rotate the blur kernel per pixel so the sparse
// tap pattern turns into fine static noise instead of visible spirals.
fn ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(0.06711056 * p.x + 0.00583715 * p.y));
}

// Disc blur: 64 Vogel-spiral taps with a gaussian-ish weight, radius `extra.xy` in uv units.
// Taps are clamped to the clip window (`grad_start`..`grad_end`) so the blur never reads
// outside the visible crop.
fn blurred(tuv: vec2<f32>, px: vec2<f32>) -> vec4<f32> {
    let n = 64u;
    let golden = 2.39996323;
    let phase = ign(px) * 6.28318530;
    var acc = vec4<f32>(0.0);
    var wsum = 0.0;
    for (var i = 0u; i < n; i = i + 1u) {
        let r = sqrt((f32(i) + 0.5) / f32(n));
        let a = f32(i) * golden + phase;
        let off = vec2<f32>(cos(a), sin(a)) * r;
        let w = exp(-2.0 * r * r);
        let uv = clamp(tuv + off * P.extra.xy, P.grad_start, P.grad_end);
        acc = acc + textureSampleLevel(tex, samp, uv, 0.0) * w;
        wsum = wsum + w;
    }
    return acc / wsum;
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Hue rotation in YIQ (the chroma plane turns, luma stays).
fn rotate_hue(c: vec3<f32>, turns: f32) -> vec3<f32> {
    let to_yiq = mat3x3<f32>(vec3<f32>(0.299, 0.596, 0.211), vec3<f32>(0.587, -0.274, -0.523), vec3<f32>(0.114, -0.322, 0.312));
    let to_rgb = mat3x3<f32>(vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(0.956, -0.272, -1.106), vec3<f32>(0.621, -0.647, 1.703));
    let yiq = to_yiq * c;
    let a = turns * 6.28318530;
    let ca = cos(a);
    let sa = sin(a);
    let r = vec3<f32>(yiq.x, ca * yiq.y - sa * yiq.z, sa * yiq.y + ca * yiq.z);
    return to_rgb * r;
}

// Camera picture pipeline: sharpen → white balance → offset → lift → gamma → gain → contrast
// (pivot) → shadows/highlights → color boost → saturation → hue → 3D LUT → grain.
fn camera_grade(sample: vec3<f32>, tuv: vec2<f32>, px: vec2<f32>) -> vec3<f32> {
    var c = sample;
    // unsharp mask over the four neighbours
    let sharpen = P.grade_c.w;
    if (sharpen > 0.0) {
        let d = P.grade_d.yz;
        let n = textureSampleLevel(tex, samp, tuv + vec2<f32>(d.x, 0.0), 0.0).rgb
              + textureSampleLevel(tex, samp, tuv - vec2<f32>(d.x, 0.0), 0.0).rgb
              + textureSampleLevel(tex, samp, tuv + vec2<f32>(0.0, d.y), 0.0).rgb
              + textureSampleLevel(tex, samp, tuv - vec2<f32>(0.0, d.y), 0.0).rgb;
        c = c + (c - n * 0.25) * sharpen * 1.5;
    }
    // temperature (warm = +red −blue) and tint (magenta = −green)
    let t = P.grade_a.x;
    let m = P.grade_a.y;
    c = c * vec3<f32>(1.0 + 0.25 * t + 0.12 * m, 1.0 - 0.2 * m, 1.0 - 0.25 * t + 0.12 * m);
    // primaries wheels
    c = c + P.grade_offset.rgb;
    c = c + P.grade_lift.rgb * (1.0 - c);
    c = pow(max(c, vec3<f32>(0.0)), P.grade_gamma.rgb);
    c = c * P.grade_gain.rgb;
    // contrast around the pivot
    c = (c - vec3<f32>(P.grade_a.w)) * P.grade_a.z + vec3<f32>(P.grade_a.w);
    // shadows / highlights: weighted by how dark / bright the pixel is
    let y0 = clamp(luma(c), 0.0, 1.0);
    c = c + vec3<f32>(P.grade_b.x * 0.3 * (1.0 - y0) * (1.0 - y0));
    c = c + vec3<f32>(P.grade_b.y * 0.3 * y0 * y0);
    // color boost (vibrance): the least saturated colours gain the most
    let y1 = luma(c);
    let sat = clamp(max(max(c.r, c.g), c.b) - min(min(c.r, c.g), c.b), 0.0, 1.0);
    c = mix(vec3<f32>(y1), c, 1.0 + P.grade_b.z * (1.0 - sat));
    // saturation
    c = mix(vec3<f32>(luma(c)), c, P.grade_b.w);
    // hue
    if (P.grade_c.x != 0.0) {
        c = rotate_hue(c, P.grade_c.x);
    }
    // 3D LUT
    if (P.grade_c.y > 0.0) {
        let n = P.grade_d.x;
        let cl = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
        let uvw = cl * (n - 1.0) / n + vec3<f32>(0.5 / n);
        let l = textureSampleLevel(lut, samp, uvw, 0.0).rgb;
        c = mix(c, l, P.grade_c.y);
    }
    // grain: static per pixel, re-seeded per frame
    if (P.grade_c.z > 0.0) {
        let g = ign(px + vec2<f32>(P.grade_d.w * 17.0, P.grade_d.w * 29.0)) - 0.5;
        c = c + vec3<f32>(g * P.grade_c.z * 0.18);
    }
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let center = P.dst.xy + P.dst.zw * 0.5;
    let half = P.dst.zw * 0.5;
    let p = in.px - center;
    let sd = sd_round_box(p, half, P.radius);
    var uv = (in.px - P.dst.xy) / P.dst.zw;
    if (P.mirror == 1u) { uv.x = 1.0 - uv.x; }
    let tuv = mix(P.uv.xy, P.uv.zw, uv);

    if (P.mode == 2u) {
        let d = P.grad_end - P.grad_start;
        let t = dot(in.px / P.out_size - P.grad_start, d) / max(dot(d, d), 1e-6);
        return vec4<f32>(gradient_color(t), 1.0) * P.alpha;
    }
    if (P.mode == 3u) {
        // Shadow body: the rect shifted by the offset, feathered over the blur radius
        // (Screen Studio: smoothstep(blur, -blur*0.5, sd)).
        let sd_body = sd_round_box(p - P.grad_start, half, P.radius);
        var k = 1.0 - smoothstep(-P.blur * 0.5, P.blur, sd_body);
        if (P.extra.x > 0.0) {
            // Directional: fade with the distance from the caster, like a light-source penumbra.
            let fall = 1.0 - clamp(sd / P.extra.x, 0.0, 1.0);
            k = k * fall * fall;
        }
        let a = k * P.alpha;
        return vec4<f32>(P.color.rgb * a, a);
    }
    let coverage = 1.0 - smoothstep(-0.5, 0.5, sd);
    if (P.mode == 0u) {
        let a = coverage * P.alpha * P.color.a;
        return vec4<f32>(P.color.rgb * a, a);
    }
    if (P.mode == 4u) {
        let s = textureSample(tex, samp, tuv);
        let inside = step(0.0, uv.x) * step(uv.x, 1.0) * step(0.0, uv.y) * step(uv.y, 1.0);
        return s * P.alpha * inside; // premultiplied sprite
    }
    if (P.mode == 5u) {
        let b = blurred(tuv, in.px);
        let rgb = mix(b.rgb, P.color.rgb, P.extra.z);
        let a = coverage * P.alpha;
        return vec4<f32>(rgb * a, a);
    }
    if (P.mode == 6u) {
        var outside = 1.0;
        for (var i = 0u; i < P.stop_count; i = i + 1u) {
            let h = P.stops[i];
            let hc = h.xy + h.zw * 0.5;
            let hsd = sd_round_box(in.px - hc, h.zw * 0.5, 0.0);
            let inside = 1.0 - smoothstep(-0.5, 0.5, hsd);
            outside = outside * (1.0 - inside);
        }
        let a = coverage * outside * P.color.a * P.alpha;
        return vec4<f32>(P.color.rgb * a, a);
    }
    if (P.mode == 7u) {
        let d = length(p);
        let r = P.extra.x;
        let w = P.extra.y;
        let disc = 1.0 - smoothstep(r - 0.75, r + 0.75, d);
        let ring = select(0.0, 1.0 - smoothstep(w, w + 1.0, abs(d - r)), w > 0.0);
        let edge = P.stops[0];
        let e = ring * edge.a;
        let fa = disc * (1.0 - e) * P.color.a;
        let a = (e + fa) * P.alpha;
        return vec4<f32>((edge.rgb * e + P.color.rgb * fa) * P.alpha, a);
    }
    if (P.mode == 8u) {
        var s: vec4<f32>;
        if (P.extra.x > 0.0 || P.extra.y > 0.0) {
            s = blurred(tuv, in.px);
        } else {
            s = textureSample(tex, samp, tuv);
        }
        return s * coverage * P.alpha;
    }
    // mode 1
    let s = textureSample(tex, samp, tuv);
    var rgb = s.rgb;
    if (P.grade == 1u) {
        rgb = camera_grade(rgb, tuv, in.px);
    }
    let a = coverage * P.alpha;
    return vec4<f32>(rgb * a, a);
}
