// Copyright 2020 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Modified by Githux, 2026-11-11: use the caller's scaled displacement once
// and sample unequal clipped input grids with transparent bounds. See
// ../../PATCH.md for the corrections and evidence.

use super::{ImageRef, ImageRefMut};
use usvg::filter::{ColorChannel, DisplacementMap};

/// Applies a displacement map.
///
/// - `map` pixels should have a **unpremultiplied alpha**.
/// - `src` pixels can have any alpha method.
///
/// `sx` and `sy` are the displacement magnitudes already transformed into canvas pixels.
///
/// Inputs outside their own grid are transparent. Filter-region rounding or
/// canvas clipping can make their dimensions differ from the output grid.
pub fn apply(
    fe: &DisplacementMap,
    sx: f32,
    sy: f32,
    src: ImageRef,
    map: ImageRef,
    dest: ImageRefMut,
) {
    let w = src.width as i32;
    let h = src.height as i32;
    let dest_width = dest.width as usize;
    for (index, output) in dest.data.iter_mut().enumerate() {
        let x = (index % dest_width) as u32;
        let y = (index / dest_width) as u32;
        let pixel = if x < map.width && y < map.height {
            map.data.get(y as usize * map.width as usize + x as usize)
        } else {
            None
        };
        let calc_offset = |channel| {
            let c = pixel.map_or(0, |pixel| match channel {
                ColorChannel::B => pixel.b,
                ColorChannel::G => pixel.g,
                ColorChannel::R => pixel.r,
                ColorChannel::A => pixel.a,
            });

            c as f32 / 255.0 - 0.5
        };

        let dx = calc_offset(fe.x_channel_selector());
        let dy = calc_offset(fe.y_channel_selector());
        let ox = (x as f32 + dx * sx).round() as i32;
        let oy = (y as f32 + dy * sy).round() as i32;

        // TODO: we should use some kind of anti-aliasing when offset is on a pixel border

        if ox >= 0 && ox < w && oy >= 0 && oy < h {
            let idx = oy as usize * src.width as usize + ox as usize;
            *output = src.data[idx];
        }
    }
}
