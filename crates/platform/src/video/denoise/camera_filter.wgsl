// Camera filter compute kernels derived from FFmpeg 7.1.5.
// ATADenoise: Copyright (c) 2015 Paul B Mahol.
// HQDN3D: Copyright (c) 2003 Daniel Moreno, 2010 Baptiste Coudurier, 2012 Loren Merritt.
// Unsharp: Copyright (c) 2002 Remi Guyomarch, 2010 Daniel G. Taylor.
// SPDX-License-Identifier: GPL-2.0-or-later
// See assets/licenses/voice/ffmpeg-COPYING.GPLv2.txt and the FFmpeg source
// and notices shipped in ffmpeg-source.

struct Params {
    dims: vec4<u32>,
    frames_a: vec4<u32>,
    frames_b: vec4<u32>,
    frames_c: vec4<u32>,
    flags: vec4<u32>,
};
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> frames: array<u32>;
@group(0) @binding(2) var<storage, read_write> temporal: array<u32>;
@group(0) @binding(3) var<storage, read_write> horizontal: array<u32>;
@group(0) @binding(4) var<storage, read_write> previous: array<u32>;
@group(0) @binding(5) var<storage, read_write> filtered: array<u32>;
@group(0) @binding(6) var<storage, read> coefficients: array<i32>;
@group(0) @binding(7) var<storage, read_write> packed: array<u32>;

fn slot(index: u32) -> u32 {
    if index < 4u { return p.frames_a[index]; }
    if index < 8u { return p.frames_b[index - 4u]; }
    return p.frames_c[index - 8u];
}
fn sample(frame: u32, index: u32) -> u32 {
    let words = (p.dims.z + 3u) / 4u;
    let word = frames[slot(frame) * words + index / 4u];
    return (word >> ((index & 3u) * 8u)) & 255u;
}
fn delta(a: u32, b: u32) -> u32 {
    return u32(abs(i32(a) - i32(b)));
}
fn lowpass(prev: u32, current: u32, table: u32) -> u32 {
    let diff = (i32(prev) - i32(current)) >> 4u;
    return u32(i32(current) + coefficients[table * 8192u + u32(diff + 4096)]);
}
fn weight(index: u32) -> u32 {
    switch index {
        case 0u, 6u: { return 1u; }
        case 1u, 5u: { return 6u; }
        case 2u, 4u: { return 15u; }
        default: { return 20u; }
    }
}

@compute @workgroup_size(64)
fn ata(@builtin(global_invocation_id) global: vec3<u32>) {
    let i = global.x;
    if i >= p.dims.z { return; }
    let center = sample(4u, i);
    var sum = center;
    var count = 1u;
    var left_sum = 0u;
    var right_sum = 0u;
    for (var distance = 1u; distance <= 4u; distance++) {
        let left = sample(4u - distance, i);
        let left_delta = delta(center, left);
        left_sum += left_delta;
        if left_delta > 4u || left_sum > 9u { break; }
        sum += left;
        count += 1u;
        let right = sample(4u + distance, i);
        let right_delta = delta(center, right);
        right_sum += right_delta;
        if right_delta > 4u || right_sum > 9u { break; }
        sum += right;
        count += 1u;
    }
    temporal[i] = (sum + count / 2u) / count;
}

@compute @workgroup_size(64)
fn horizontal_hq(@builtin(global_invocation_id) global: vec3<u32>) {
    var row = global.x;
    let height = p.dims.y;
    var width = p.dims.x;
    var base = 0u;
    var table = 0u;
    if row >= height {
        row -= height;
        width /= 2u;
        base = p.dims.w;
        table = 1u;
        if row >= height / 2u {
            row -= height / 2u;
            base += p.dims.w / 4u;
        }
        if row >= height / 2u { return; }
    }
    let start = base + row * width;
    var last = temporal[start] * 256u + 127u;
    if row == 0u { last = lowpass(last, last, table); }
    horizontal[start] = last;
    for (var x = 1u; x < width; x++) {
        let current = temporal[start + x] * 256u + 127u;
        last = lowpass(last, current, table);
        horizontal[start + x] = last;
    }
}

@compute @workgroup_size(64)
fn vertical_hq(@builtin(global_invocation_id) global: vec3<u32>) {
    var col = global.x;
    let width = p.dims.x;
    var w = width;
    var height = p.dims.y;
    var base = 0u;
    var spatial = 0u;
    var time_table = 2u;
    if col >= width {
        col -= width;
        w /= 2u;
        height /= 2u;
        base = p.dims.w;
        spatial = 1u;
        time_table = 3u;
        if col >= w {
            col -= w;
            base += p.dims.w / 4u;
        }
        if col >= w { return; }
    }
    var line = 0u;
    for (var y = 0u; y < height; y++) {
        let i = base + y * w + col;
        let value = horizontal[i];
        if y == 0u { line = value; } else { line = lowpass(line, value, spatial); }
        var old = previous[i];
        if p.flags.x != 0u { old = temporal[i] * 256u + 127u; }
        let next = lowpass(old, line, time_table);
        previous[i] = next;
        filtered[i] = next >> 8u;
    }
}

@compute @workgroup_size(64)
fn blur_horizontal(@builtin(global_invocation_id) global: vec3<u32>) {
    let i = global.x;
    if i >= p.dims.w { return; }
    let width = p.dims.x;
    let y = i / width;
    let x = i % width;
    var sum = 0u;
    for (var j = 0u; j < 7u; j++) {
        let sx = u32(clamp(i32(x) + i32(j) - 3, 0, i32(width) - 1));
        sum += filtered[y * width + sx] * weight(j);
    }
    horizontal[i] = sum;
}

@compute @workgroup_size(64)
fn sharpen(@builtin(global_invocation_id) global: vec3<u32>) {
    let word = global.x;
    if word * 4u >= p.dims.z { return; }
    var result = 0u;
    for (var channel = 0u; channel < 4u; channel++) {
        let i = word * 4u + channel;
        if i >= p.dims.z { break; }
        var value = filtered[i];
        if i < p.dims.w {
            let width = p.dims.x;
            let x = i % width;
            let y = i / width;
            var sum = 0u;
            for (var j = 0u; j < 7u; j++) {
                let sy = u32(clamp(i32(y) + i32(j) - 3, 0, i32(p.dims.y) - 1));
                sum += horizontal[sy * width + x] * weight(j);
            }
            let blurred = i32((sum + 2048u) >> 12u);
            let source = i32(value);
            value = u32(clamp(source + ((source - blurred) >> 1u), 0, 255));
        }
        result |= value << (channel * 8u);
    }
    packed[word] = result;
}
