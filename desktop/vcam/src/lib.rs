// SPDX-License-Identifier: GPL-3.0-or-later
//! `g7link_vcam`: Windows 11 Media Foundation virtual camera media source
//! COM DLL and shared-memory frame transport (`docs/protocol/webcam.md`).

#![allow(linker_messages)]



pub mod shm;

pub use shm::{
    DEFAULT_MAPPING_NAME, LOCAL_MAPPING_NAME, MAX_FRAME_BYTES, MAX_HEIGHT, MAX_WIDTH, SharedFrameMapping,
    bgrx_to_nv12, fit_bgrx_aspect, render_placeholder_bgrx,
};

/// CLSID of the G7 Link Virtual Camera Media Foundation media source:
/// `{8E6C3B74-5D4A-4B9E-9A12-7C8F1E2D3A40}`.

/// Standard COM DLL entry point returning an `IClassFactory` for
/// [`CLSID_G7LINK_VCAM`].
///
/// # Safety
/// `rclsid`, `riid`, and `ppv` must be valid pointers as specified by COM.

/// Standard COM DLL unload check.

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;




    #[test]
    fn shared_memory_mirror_and_aspect_fit_round_trip() {
        let map_name = format!("Local\\G7LinkVcamShmTest_{}", std::process::id());
        let writer = SharedFrameMapping::open_or_create(&map_name).unwrap();
        let reader = SharedFrameMapping::open_or_create(&map_name).unwrap();

        // 4x2 frame where left half is blue and right half is red.
        let mut src = vec![0u8; 4 * 2 * 4];
        for y in 0..2 {
            for x in 0..4 {
                let off = (y * 4 + x) * 4;
                if x < 2 {
                    src[off..off + 4].copy_from_slice(&[255, 0, 0, 255]);
                } else {
                    src[off..off + 4].copy_from_slice(&[0, 0, 255, 255]);
                }
            }
        }
        writer.write_frame("Pixel 8", 4, 2, 999, &src, true);

        let mut dst = vec![0u8; 4 * 2 * 4];
        let ts = reader.read_bgrx(4, 2, &mut dst);
        assert_eq!(ts, (true, 999));
        // Because mirror=true, left half is now red and right half is blue.
        assert_eq!(&dst[0..4], &[0, 0, 255, 255]);
        assert_eq!(&dst[8..12], &[255, 0, 0, 255]);
    }
}
