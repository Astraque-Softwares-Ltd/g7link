// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared perceptual color extraction and Bloom tonal scheme builder,
//! re-exported from [`g7link_core::palette`].

pub use g7link_core::palette::{
    MAX_SAMPLE_SIDE, extract_artwork_scheme_rgb, scheme_json, seed_from_grid, seed_from_pixels,
};
