// SPDX-License-Identifier: GPL-3.0-or-later
//! Shares the PC's display with a paired phone over `mirror` / `video` (session 0).
//! Encodes H.264 or HEVC with FFmpeg or OpenH264, and handles remote touch,
//! keyboard, and pointer input.

mod imp {

    #![allow(unsafe_code)]

    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    };
    use std::time::{Duration, Instant};

    use g7link_core::{CapturePrompt, DeviceId, KeyMod, MirrorInput, MirrorStart, PcCaptureError, mirror_keys};
    use g7link_core::{MirrorConfig, MirrorSend, PacketKind};

    use crate::{core_host, sys};

    struct ActiveSession {
        peer: DeviceId,
        stop: Arc<AtomicBool>,
        force_keyframe: Arc<AtomicBool>,
        /// Play on phone: the phone watches a player, and its input is ignored.
        watching: bool,
    }

    static ACTIVE: Mutex<Option<ActiveSession>> = Mutex::new(None);

    /// How long Play on phone looks for the player's window before streaming
    /// the screen instead.
    const FIND_WINDOW: Duration = Duration::from_secs(5);

    /// The lowest bitrate the stream backs off to on a slow link (encoders
    /// go no lower).
    const MIN_BITRATE: u32 = 500_000;
    /// A measure of the link over fewer bytes than this says how late they
    /// were, not what the link carries (a quiet screen, the phone's radio
    /// asleep for a moment): the bitrate stays.
    const MIN_MEASURED_BYTES: u64 = 16 * 1024;
    /// How long the link must have carried everything before the bitrate
    /// steps back up, and how often it then does, a quarter at a time. From
    /// the lowest bitrate to 4 Mbit/s that is about half a minute; it used to
    /// be over a minute and a half.
    const RAISE_AFTER_CLEAN: Duration = Duration::from_secs(4);
    const RAISE_EVERY: Duration = Duration::from_secs(3);

    /// What a sharing session sent in the last stats window.
    #[derive(Default)]
    struct Stats {
        frames: u64,
        keyframes: u64,
        bytes: u64,
        dropped: u64,
        rebuilds: u64,
    }

    /// Returns the device currently viewing this PC's screen, if any.
    pub fn active_peer() -> Option<DeviceId> {
        ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.peer)
    }

    /// Whether this PC's screen is currently being shared with a phone.
    pub fn is_active() -> bool {
        active_peer().is_some()
    }

    /// Whether this PC's display session supports screen capture for `mirror.capture`.
    ///
    /// Linux needs an H.264 encoder and either X11 or the ScreenCast portal with PipeWire.
    pub fn is_capture_available() -> bool {
        {
            let can_encode = openh264_enc_api().is_some();
            #[cfg(linux_ffmpeg_video)]
            let can_encode = can_encode || {
                static HARDWARE_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                *HARDWARE_AVAILABLE.get_or_init(|| {
                    crate::sys::video_encoder::Encoder::new("h264", 320, 240, 30, 1_000_000).is_ok()
                })
            };
            if !can_encode {
                return false;
            }
            let has_x11 = std::env::var_os("DISPLAY").is_some() && x11_api().is_some();
            has_x11 || is_wayland_screencast_available()
        }
    }

    /// Whether a phone watches one of this PC's players (Play on phone).
    pub fn is_watching() -> bool {
        ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.watching)
    }

    /// Stops a phone watching one of this PC's players. That ends the
    /// session: a watching session is the one capture there is.
    pub fn stop_watching() {
        let peer =
            ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|s| s.watching).map(|s| s.peer);
        if let Some(peer) = peer {
            stop_for(&peer);
        }
    }

    /// Starts or replaces the active PC-to-phone screen mirror stream for `peer`.
    pub fn start(peer: DeviceId, options: MirrorStart) {
        // The same phone asking again replaced its own stream: telling it to
        // stop could land after the new one starts and end that.
        stop_session(Some(peer));
        let stop = Arc::new(AtomicBool::new(false));
        let force_keyframe = Arc::new(AtomicBool::new(false));
        {
            let mut guard = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some(ActiveSession {
                peer,
                stop: Arc::clone(&stop),
                force_keyframe: Arc::clone(&force_keyframe),
                watching: options.watch.is_some(),
            });
        }
        core_host::host().hub.changed(crate::state::Changes::REMOTE);
        core_host::refresh_capabilities();

        let stop_on_failure = Arc::clone(&stop);
        if let Err(e) = std::thread::Builder::new()
            .name("g7link-pc-mirror".into())
            .spawn(move || run_capture_loop(peer, options, stop, force_keyframe))
        {
            // Not a session that claims to share with nothing behind it.
            tracing::warn!(error = %e, "can't start sharing this PC's screen");
            cleanup_session(&stop_on_failure);
        }
    }

    /// Stops PC-to-phone mirroring if `peer` matches the active viewer.
    pub fn stop_for(peer: &DeviceId) {
        let stopped = {
            let mut guard = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
            if guard.as_ref().is_some_and(|s| &s.peer == peer)
                && let Some(s) = guard.take()
            {
                s.stop.store(true, Ordering::SeqCst);
                true
            } else {
                false
            }
        };
        if stopped {
            sys::input::release_held_buttons();
            crate::input_share::leave_remote();
            core_host::host().hub.changed(crate::state::Changes::REMOTE);
            core_host::refresh_capabilities();
        }
    }

    /// Stops any active PC-to-phone mirroring session.
    pub fn stop_all() {
        stop_session(None);
    }

    /// Stops the active session, telling its phone unless it's `replaced_by`.
    fn stop_session(replaced_by: Option<DeviceId>) {
        let stopped = {
            let mut guard = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(s) = guard.take() {
                s.stop.store(true, Ordering::SeqCst);
                Some(s.peer)
            } else {
                None
            }
        };
        if let Some(peer) = stopped {
            sys::input::release_held_buttons();
            crate::input_share::leave_remote();
            if let Some(node) = core_host::node()
                && replaced_by != Some(peer)
            {
                node.set_pc_mirror_session_allowed(peer, false);
                core_host::spawn(async move {
                    node.mirror_stop(peer, g7link_core::MIRROR_SCREEN).await;
                });
            }
            core_host::host().hub.changed(crate::state::Changes::REMOTE);
            core_host::refresh_capabilities();
        }
    }

    /// Requests an immediate IDR keyframe for `peer`.
    pub fn request_keyframe(peer: &DeviceId) {
        let guard = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = guard.as_ref()
            && &s.peer == peer
        {
            s.force_keyframe.store(true, Ordering::SeqCst);
        }
    }

    /// Injects touch, scroll, key, or text input from a phone viewing the PC screen.
    pub fn handle_input(peer: &DeviceId, session: u32, input: MirrorInput) {
        if session != g7link_core::MIRROR_SCREEN {
            return;
        }
        if active_peer() != Some(*peer) || is_watching() {
            return;
        }
        let remote_input_on = core_host::node()
            .and_then(|n| n.device_toggles(*peer).ok())
            .is_some_and(|toggles| toggles.iter().any(|(k, v)| *k == "remote_input" && *v));
        if !remote_input_on {
            return;
        }
        let _ = crate::input_share::enter_remote(*peer);
        match input {
            MirrorInput::Touch { action, x, y } => {
                sys::input::mirror_touch(action, x, y);
            }
            MirrorInput::Scroll { x, y, dx, dy } => {
                sys::input::move_absolute(x, y);
                sys::input::scroll(dx, dy);
            }
            MirrorInput::Text { text } => {
                sys::input::type_text(&text);
            }
            MirrorInput::Key { key } => match key.as_str() {
                mirror_keys::BACK => sys::input::press_key("escape", &[]),
                mirror_keys::HOME => sys::input::press_key("a", &[KeyMod::Win]),
                mirror_keys::RECENTS => sys::input::press_key("task_view", &[]),
                mirror_keys::ENTER => sys::input::press_key("enter", &[]),
                mirror_keys::BACKSPACE => sys::input::press_key("backspace", &[]),
                mirror_keys::DELETE => sys::input::press_key("delete", &[]),
                mirror_keys::LEFT => sys::input::press_key("left", &[]),
                mirror_keys::RIGHT => sys::input::press_key("right", &[]),
                mirror_keys::UP => sys::input::press_key("up", &[]),
                mirror_keys::DOWN => sys::input::press_key("down", &[]),
                mirror_keys::TAB => sys::input::press_key("tab", &[]),
                mirror_keys::NOTIFICATIONS => sys::input::press_key("n", &[KeyMod::Win]),
                _ => {}
            },
            MirrorInput::Gamepad { .. } => {}
        }
    }

    pub fn on_event(event: &g7link_core::NodeEvent) {
        match event {
            g7link_core::NodeEvent::LinkChanged { device, link: g7link_core::LinkState::Offline { .. } }
            | g7link_core::NodeEvent::DeviceRemoved(device) => {
                stop_for(device);
            }
            g7link_core::NodeEvent::PcMirrorRequested { .. } => {
                core_host::host().hub.update(|s| s.apply(event));
                crate::bridge::app::request_activation();
            }
            _ => {}
        }
    }

    /// Computes target encoder dimensions (multiples of 16) fitting within `max_size`.
    pub(crate) fn target_dimensions(src_w: u32, src_h: u32, max_size: u32) -> (u32, u32) {
        let max_edge = max_size.clamp(320, 1920) as f64;
        let sw = src_w.max(64) as f64;
        let sh = src_h.max(64) as f64;
        let longest = sw.max(sh);
        let scale = if longest > max_edge { max_edge / longest } else { 1.0 };
        let w = (((sw * scale).round() as u32) / 16).max(4) * 16;
        let h = (((sh * scale).round() as u32) / 16).max(4) * 16;
        (w, h)
    }

    /// Converts a top-down 32-bit BGRA image (`width * height * 4` bytes) into
    /// BT.709 limited-range NV12 (`width * height * 3 / 2` bytes).
    pub(crate) fn bgra_to_nv12(bgra: &[u8], width: usize, height: usize, nv12: &mut Vec<u8>) {
        let y_size = width * height;
        let uv_size = y_size / 2;
        nv12.resize(y_size + uv_size, 0);
        let (y_plane, uv_plane) = nv12.split_at_mut(y_size);

        for row in 0..height {
            let src_row = row * width * 4;
            let y_row = row * width;
            for col in 0..width {
                let px = src_row + col * 4;
                let b = i32::from(bgra[px]);
                let g = i32::from(bgra[px + 1]);
                let r = i32::from(bgra[px + 2]);
                let y = ((47 * r + 157 * g + 16 * b + 128) >> 8) + 16;
                y_plane[y_row + col] = y.clamp(16, 235) as u8;
            }
        }

        for row in (0..height).step_by(2) {
            let src0 = row * width * 4;
            let src1 = (row + 1).min(height - 1) * width * 4;
            let uv_row = (row / 2) * width;
            for col in (0..width).step_by(2) {
                let p00 = src0 + col * 4;
                let p01 = src0 + (col + 1).min(width - 1) * 4;
                let p10 = src1 + col * 4;
                let p11 = src1 + (col + 1).min(width - 1) * 4;

                let b = (i32::from(bgra[p00])
                    + i32::from(bgra[p01])
                    + i32::from(bgra[p10])
                    + i32::from(bgra[p11])
                    + 2)
                    >> 2;
                let g = (i32::from(bgra[p00 + 1])
                    + i32::from(bgra[p01 + 1])
                    + i32::from(bgra[p10 + 1])
                    + i32::from(bgra[p11 + 1])
                    + 2)
                    >> 2;
                let r = (i32::from(bgra[p00 + 2])
                    + i32::from(bgra[p01 + 2])
                    + i32::from(bgra[p10 + 2])
                    + i32::from(bgra[p11 + 2])
                    + 2)
                    >> 2;

                let u = ((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128;
                let v = ((112 * r - 102 * g - 10 * b + 128) >> 8) + 128;
                uv_plane[uv_row + col] = u.clamp(16, 240) as u8;
                uv_plane[uv_row + col + 1] = v.clamp(16, 240) as u8;
            }
        }
    }

    fn has_h264_idr(data: &[u8]) -> bool {
        data.windows(4).any(|w| w[..3] == [0, 0, 1] && (w[3] & 0x1f) == 5)
    }

    fn has_h264_sps(data: &[u8]) -> bool {
        data.windows(4).any(|w| w[..3] == [0, 0, 1] && (w[3] & 0x1f) == 7)
    }

    #[cfg(test)]
    fn hevc_nal_types(data: &[u8]) -> impl Iterator<Item = u8> + '_ {
        data.windows(4).filter_map(|w| (w[..3] == [0, 0, 1]).then_some((w[3] >> 1) & 0x3f))
    }

    #[cfg(test)]
    fn has_h265_irap(data: &[u8]) -> bool {
        hevc_nal_types(data).any(|kind| (16..=21).contains(&kind))
    }

    #[cfg(test)]
    fn has_h265_parameters(data: &[u8]) -> bool {
        let mut vps = false;
        let mut sps = false;
        let mut pps = false;
        for kind in hevc_nal_types(data) {
            match kind {
                32 => vps = true,
                33 => sps = true,
                34 => pps = true,
                _ => {}
            }
        }
        vps && sps && pps
    }

    #[repr(C)]
    struct SEncParamBase {
        i_usage_type: std::ffi::c_int, // 0 = CAMERA_VIDEO_REAL_TIME
        i_pic_width: std::ffi::c_int,
        i_pic_height: std::ffi::c_int,
        i_target_bitrate: std::ffi::c_int,
        i_rc_mode: std::ffi::c_int, // 1 = RC_BITRATE_MODE
        f_max_frame_rate: f32,
    }

    #[repr(C)]
    struct SSourcePicture {
        i_color_format: std::ffi::c_int, // 23 = videoFormatI420
        i_stride: [std::ffi::c_int; 4],
        p_data: [*mut u8; 4],
        i_pic_width: std::ffi::c_int,
        i_pic_height: std::ffi::c_int,
        ui_time_stamp: i64,
        b_psnr_y: bool,
        b_psnr_u: bool,
        b_psnr_v: bool,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SLayerBSInfoV20 {
        ui_temporal_id: u8,
        ui_spatial_id: u8,
        ui_quality_id: u8,
        e_frame_type: std::ffi::c_int,
        ui_layer_type: u8,
        i_sub_seq_id: std::ffi::c_int,
        i_nal_count: std::ffi::c_int,
        p_nal_length_in_byte: *mut std::ffi::c_int,
        p_bs_buf: *mut u8,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SLayerBSInfo {
        ui_temporal_id: u8,
        ui_spatial_id: u8,
        ui_quality_id: u8,
        e_frame_type: std::ffi::c_int,
        ui_layer_type: u8,
        i_sub_seq_id: std::ffi::c_int,
        i_nal_count: std::ffi::c_int,
        p_nal_length_in_byte: *mut std::ffi::c_int,
        p_bs_buf: *mut u8,
        r_psnr: [f32; 3],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SFrameBSInfoV20 {
        i_layer_num: std::ffi::c_int,
        s_layer_info: [SLayerBSInfoV20; 128],
        e_frame_type: std::ffi::c_int, // 1 = IDR, 2 = I, 3 = P, 4 = Skip
        i_frame_size_in_bytes: std::ffi::c_int,
        ui_time_stamp: i64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SFrameBSInfo {
        i_layer_num: std::ffi::c_int,
        s_layer_info: [SLayerBSInfo; 128],
        e_frame_type: std::ffi::c_int, // 1 = IDR, 2 = I, 3 = P, 4 = Skip
        i_frame_size_in_bytes: std::ffi::c_int,
        ui_time_stamp: i64,
    }

    /// Upstream OpenH264 2.3+ adds `r_psnr: [f32; 3]` to `SLayerBSInfo`, whereas
    /// Debian/Ubuntu's `libopenh264.so.7` (2.4.1) preserves the 40-byte `so.7`
    /// `SLayerBSInfoV20` layout despite reporting `u_minor == 4`. Zero-initializing
    /// this union before `EncodeFrame` lets us inspect whether `v23.e_frame_type`
    /// (at byte 7176, past the end of `SFrameBSInfoV20`) was populated.
    #[repr(C)]
    union FrameBSInfo {
        v20: SFrameBSInfoV20,
        v23: SFrameBSInfo,
    }

    #[repr(C)]
    struct SBitrateInfo {
        i_layer: std::ffi::c_int, // 4 = SPATIAL_LAYER_ALL, 0 = SPATIAL_LAYER_0
        i_bitrate: std::ffi::c_int,
    }

    #[repr(C)]
    struct OpenH264Version {
        u_major: u32,
        u_minor: u32,
        u_revision: u32,
        u_reserved: u32,
    }

    struct OpenH264EncApi {
        minor: u32,
        create_encoder: unsafe extern "C" fn(*mut *mut std::ffi::c_void) -> std::ffi::c_int,
        destroy_encoder: unsafe extern "C" fn(*mut std::ffi::c_void),
    }

    unsafe fn dl_sym<T: Copy>(lib: *mut std::ffi::c_void, name: &std::ffi::CStr) -> Option<T> {
        let raw = unsafe { libc::dlsym(lib, name.as_ptr()) };
        if raw.is_null() { None } else { Some(unsafe { std::mem::transmute_copy(&raw) }) }
    }

    fn openh264_enc_api() -> Option<&'static OpenH264EncApi> {
        static API: std::sync::OnceLock<Option<OpenH264EncApi>> = std::sync::OnceLock::new();
        API.get_or_init(|| unsafe {
            let candidates =
                [c"libopenh264.so.8", c"libopenh264.so.7", c"libopenh264.so.6", c"libopenh264.so"];
            let mut lib = std::ptr::null_mut();
            for name in candidates {
                lib = libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
                if !lib.is_null() {
                    break;
                }
            }
            if lib.is_null() {
                return None;
            }
            let get_version: unsafe extern "C" fn() -> OpenH264Version = dl_sym(lib, c"WelsGetCodecVersion")?;
            let ver = get_version();
            if ver.u_major != 2 {
                tracing::warn!(
                    major = ver.u_major,
                    minor = ver.u_minor,
                    "unsupported OpenH264 ABI version; skipping OpenH264 encoder"
                );
                return None;
            }
            Some(OpenH264EncApi {
                minor: ver.u_minor,
                create_encoder: dl_sym(lib, c"WelsCreateSVCEncoder")?,
                destroy_encoder: dl_sym(lib, c"WelsDestroySVCEncoder")?,
            })
        })
        .as_ref()
    }

    pub(crate) struct OpenH264Encoder {
        encoder: *mut std::ffi::c_void,
        width: u32,
        height: u32,
        i420_buf: Vec<u8>,
        seq_header: Vec<u8>,
        sent_first_frame: bool,
    }

    impl Drop for OpenH264Encoder {
        fn drop(&mut self) {
            if !self.encoder.is_null() {
                unsafe {
                    let vtable = *(self.encoder as *const *const usize);
                    let uninitialize: unsafe extern "C" fn(*mut std::ffi::c_void) -> std::ffi::c_int =
                        std::mem::transmute(*vtable.add(3));
                    let _ = uninitialize(self.encoder);
                    if let Some(api) = openh264_enc_api() {
                        (api.destroy_encoder)(self.encoder);
                    }
                }
            }
        }
    }

    impl OpenH264Encoder {
        pub(crate) fn new(width: u32, height: u32, fps: u32, bitrate: u32) -> Result<Self, String> {
            let api = openh264_enc_api().ok_or_else(|| "libopenh264 encoder unavailable".to_owned())?;
            unsafe {
                let mut encoder: *mut std::ffi::c_void = std::ptr::null_mut();
                if (api.create_encoder)(&mut encoder) != 0 || encoder.is_null() {
                    return Err("WelsCreateSVCEncoder failed".to_owned());
                }
                let vtable = *(encoder as *const *const usize);
                let initialize: unsafe extern "C" fn(
                    *mut std::ffi::c_void,
                    *const SEncParamBase,
                ) -> std::ffi::c_int = std::mem::transmute(*vtable.add(0));
                let set_option: unsafe extern "C" fn(
                    *mut std::ffi::c_void,
                    std::ffi::c_int,
                    *mut std::ffi::c_void,
                ) -> std::ffi::c_int = std::mem::transmute(*vtable.add(7));

                let mut trace_level: std::ffi::c_int = 0;
                let _ = set_option(
                    encoder,
                    25, // ENCODER_OPTION_TRACE_LEVEL
                    (&mut trace_level as *mut std::ffi::c_int).cast(),
                );

                let param = SEncParamBase {
                    i_usage_type: 0, // CAMERA_VIDEO_REAL_TIME
                    i_pic_width: width as std::ffi::c_int,
                    i_pic_height: height as std::ffi::c_int,
                    i_target_bitrate: bitrate.clamp(500_000, 20_000_000) as std::ffi::c_int,
                    i_rc_mode: 1, // RC_BITRATE_MODE
                    f_max_frame_rate: fps.max(1) as f32,
                };
                let rc = initialize(encoder, &param);
                if rc != 0 {
                    (api.destroy_encoder)(encoder);
                    return Err(format!("OpenH264 Initialize failed with code {rc}"));
                }

                let mut idr_interval = (fps.max(1) * 2) as std::ffi::c_int;
                let _ = set_option(
                    encoder,
                    1, // ENCODER_OPTION_IDR_INTERVAL
                    (&mut idr_interval as *mut std::ffi::c_int).cast(),
                );

                Ok(Self {
                    encoder,
                    width,
                    height,
                    i420_buf: Vec::new(),
                    seq_header: Vec::new(),
                    sent_first_frame: false,
                })
            }
        }

        pub(crate) fn request_keyframe(&self) -> bool {
            if self.encoder.is_null() {
                return false;
            }
            unsafe {
                let vtable = *(self.encoder as *const *const usize);
                let force_intra_frame: unsafe extern "C" fn(
                    *mut std::ffi::c_void,
                    bool,
                    std::ffi::c_int,
                ) -> std::ffi::c_int = std::mem::transmute(*vtable.add(6));
                force_intra_frame(self.encoder, true, -1) == 0
            }
        }

        pub(crate) fn set_bitrate(&self, bitrate: u32) -> bool {
            if self.encoder.is_null() {
                return false;
            }
            unsafe {
                let vtable = *(self.encoder as *const *const usize);
                let set_option: unsafe extern "C" fn(
                    *mut std::ffi::c_void,
                    std::ffi::c_int,
                    *mut std::ffi::c_void,
                ) -> std::ffi::c_int = std::mem::transmute(*vtable.add(7));
                let target = bitrate.clamp(500_000, 20_000_000) as std::ffi::c_int;
                let mut info_all = SBitrateInfo {
                    i_layer: 4, // SPATIAL_LAYER_ALL
                    i_bitrate: target,
                };
                let mut info_0 = SBitrateInfo {
                    i_layer: 0, // SPATIAL_LAYER_0
                    i_bitrate: target,
                };
                let rc_all = set_option(
                    self.encoder,
                    5, // ENCODER_OPTION_BITRATE
                    (&mut info_all as *mut SBitrateInfo).cast(),
                );
                let rc_0 = set_option(
                    self.encoder,
                    5, // ENCODER_OPTION_BITRATE
                    (&mut info_0 as *mut SBitrateInfo).cast(),
                );
                rc_all == 0 || rc_0 == 0
            }
        }

        pub(crate) fn encode(
            &mut self,
            nv12: &[u8],
            hns: i64,
            _dur_hns: i64,
            stop: &AtomicBool,
        ) -> Result<Vec<(bool, Vec<u8>)>, String> {
            let w = self.width as usize;
            let h = self.height as usize;
            let y_size = w * h;
            let uv_size = y_size / 2;
            let expected = y_size + uv_size;
            if stop.load(Ordering::Relaxed) || nv12.len() < expected || self.encoder.is_null() {
                return Ok(Vec::new());
            }

            let quarter = y_size / 4;
            self.i420_buf.resize(expected, 0);
            self.i420_buf[..y_size].copy_from_slice(&nv12[..y_size]);
            {
                let (y_and_u, v_plane) = self.i420_buf.split_at_mut(y_size + quarter);
                let u_plane = &mut y_and_u[y_size..];
                let src_uv = &nv12[y_size..expected];
                for i in 0..quarter {
                    u_plane[i] = src_uv[i * 2];
                    v_plane[i] = src_uv[i * 2 + 1];
                }
            }

            unsafe {
                let api = openh264_enc_api().ok_or_else(|| "libopenh264 encoder unavailable".to_owned())?;
                let y_ptr = self.i420_buf.as_mut_ptr();
                let u_ptr = y_ptr.add(y_size);
                let v_ptr = y_ptr.add(y_size + quarter);
                let pic = SSourcePicture {
                    i_color_format: 23, // videoFormatI420
                    i_stride: [
                        w as std::ffi::c_int,
                        (w / 2) as std::ffi::c_int,
                        (w / 2) as std::ffi::c_int,
                        0,
                    ],
                    p_data: [y_ptr, u_ptr, v_ptr, std::ptr::null_mut()],
                    i_pic_width: w as std::ffi::c_int,
                    i_pic_height: h as std::ffi::c_int,
                    ui_time_stamp: hns / 10_000,
                    b_psnr_y: false,
                    b_psnr_u: false,
                    b_psnr_v: false,
                };

                let vtable = *(self.encoder as *const *const usize);
                let mut raw = Vec::new();
                let e_frame_type: std::ffi::c_int;

                let mut bs_union: FrameBSInfo = std::mem::zeroed();
                let encode_frame: unsafe extern "C" fn(
                    *mut std::ffi::c_void,
                    *const SSourcePicture,
                    *mut FrameBSInfo,
                ) -> std::ffi::c_int = std::mem::transmute(*vtable.add(4));

                let rc = encode_frame(self.encoder, &pic, &mut bs_union);
                if rc != 0 {
                    return Err(format!("OpenH264 EncodeFrame failed: {rc}"));
                }

                if api.minor >= 3 && bs_union.v23.e_frame_type != 0 {
                    let bs_info = &bs_union.v23;
                    if bs_info.e_frame_type == 4 || bs_info.i_layer_num <= 0 {
                        return Ok(Vec::new());
                    }
                    e_frame_type = bs_info.e_frame_type;
                    let layer_count = (bs_info.i_layer_num as usize).min(bs_info.s_layer_info.len());
                    for idx in 0..layer_count {
                        let layer = &bs_info.s_layer_info[idx];
                        if layer.p_bs_buf.is_null()
                            || layer.p_nal_length_in_byte.is_null()
                            || layer.i_nal_count <= 0
                        {
                            continue;
                        }
                        let nal_lens = std::slice::from_raw_parts(
                            layer.p_nal_length_in_byte,
                            layer.i_nal_count as usize,
                        );
                        let total_bytes: usize = nal_lens.iter().map(|&n| n.max(0) as usize).sum();
                        if total_bytes == 0 {
                            continue;
                        }
                        let layer_bytes = std::slice::from_raw_parts(layer.p_bs_buf, total_bytes);
                        if layer.ui_layer_type == 0 {
                            self.seq_header.clear();
                            self.seq_header.extend_from_slice(layer_bytes);
                        }
                        raw.extend_from_slice(layer_bytes);
                    }
                } else {
                    let bs_info = &bs_union.v20;
                    if bs_info.e_frame_type == 4 || bs_info.i_layer_num <= 0 {
                        return Ok(Vec::new());
                    }
                    e_frame_type = bs_info.e_frame_type;
                    let layer_count = (bs_info.i_layer_num as usize).min(bs_info.s_layer_info.len());
                    for idx in 0..layer_count {
                        let layer = &bs_info.s_layer_info[idx];
                        if layer.p_bs_buf.is_null()
                            || layer.p_nal_length_in_byte.is_null()
                            || layer.i_nal_count <= 0
                        {
                            continue;
                        }
                        let nal_lens = std::slice::from_raw_parts(
                            layer.p_nal_length_in_byte,
                            layer.i_nal_count as usize,
                        );
                        let total_bytes: usize = nal_lens.iter().map(|&n| n.max(0) as usize).sum();
                        if total_bytes == 0 {
                            continue;
                        }
                        let layer_bytes = std::slice::from_raw_parts(layer.p_bs_buf, total_bytes);
                        if layer.ui_layer_type == 0 {
                            self.seq_header.clear();
                            self.seq_header.extend_from_slice(layer_bytes);
                        }
                        raw.extend_from_slice(layer_bytes);
                    }
                }

                if raw.is_empty() {
                    return Ok(Vec::new());
                }

                let is_idr = !self.sent_first_frame || e_frame_type == 1 || has_h264_idr(&raw);
                let mut bytes = Vec::new();
                if is_idr && !has_h264_sps(&raw) && !self.seq_header.is_empty() {
                    bytes.extend_from_slice(&self.seq_header);
                }
                bytes.extend_from_slice(&raw);
                self.sent_first_frame = true;
                Ok(vec![(is_idr, bytes)])
            }
        }
    }

    pub(crate) enum ScreenEncoder {
        OpenH264(OpenH264Encoder),
        #[cfg(linux_ffmpeg_video)]
        Hardware(crate::sys::video_encoder::Encoder),
    }

    impl ScreenEncoder {
        pub(crate) fn new_for_codec(
            codec: &str,
            width: u32,
            height: u32,
            fps: u32,
            bitrate: u32,
        ) -> Result<Self, String> {
            #[cfg(linux_ffmpeg_video)]
            match crate::sys::video_encoder::Encoder::new(codec, width, height, fps, bitrate) {
                Ok(encoder) => return Ok(Self::Hardware(encoder)),
                Err(error) if codec != "h264" => return Err(error),
                Err(error) => tracing::debug!(%error, "Hardware H.264 unavailable; using OpenH264"),
            }
            if codec != "h264" {
                return Err(
                    "HEVC hardware encoding requires FFmpeg development libraries at build time".into()
                );
            }
            OpenH264Encoder::new(width, height, fps, bitrate).map(Self::OpenH264)
        }

        #[cfg(linux_ffmpeg_video)]
        fn next_encoder(&mut self, codec: &str, width: u32, height: u32, fps: u32, bitrate: u32) -> bool {
            if let Self::Hardware(encoder) = self {
                if encoder.next_device() {
                    return true;
                }
                if codec == "h264"
                    && let Ok(software) = OpenH264Encoder::new(width, height, fps, bitrate)
                {
                    *self = Self::OpenH264(software);
                    return true;
                }
            }
            false
        }

        pub(crate) fn request_keyframe(&self) -> bool {
            match self {
                Self::OpenH264(encoder) => encoder.request_keyframe(),
                #[cfg(linux_ffmpeg_video)]
                Self::Hardware(encoder) => encoder.request_keyframe(),
            }
        }

        pub(crate) fn set_bitrate(&self, bitrate: u32) -> bool {
            match self {
                Self::OpenH264(encoder) => encoder.set_bitrate(bitrate),
                #[cfg(linux_ffmpeg_video)]
                Self::Hardware(encoder) => encoder.set_bitrate(bitrate),
            }
        }

        pub(crate) fn encode(
            &mut self,
            nv12: &[u8],
            hns: i64,
            dur_hns: i64,
            stop: &AtomicBool,
        ) -> Result<Vec<(bool, Vec<u8>)>, String> {
            match self {
                Self::OpenH264(encoder) => encoder.encode(nv12, hns, dur_hns, stop),
                #[cfg(linux_ffmpeg_video)]
                Self::Hardware(encoder) => encoder.encode(nv12, hns, stop),
            }
        }
    }

    fn new_pc_encoder(
        codec: &str,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
    ) -> Result<ScreenEncoder, String> {
        ScreenEncoder::new_for_codec(codec, width, height, fps, bitrate)
    }

    fn new_pc_encoder_with_fallback(
        codec: &mut String,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
    ) -> Result<ScreenEncoder, String> {
        match new_pc_encoder(codec, width, height, fps, bitrate) {
            Ok(encoder) => Ok(encoder),
            Err(error) if codec == "h265" => {
                tracing::info!(%error, "HEVC screen encoder unavailable; using H.264");
                *codec = "h264".into();
                new_pc_encoder(codec, width, height, fps, bitrate)
            }
            Err(error) => Err(error),
        }
    }

    #[repr(C)]
    struct XWindowAttributes {
        x: std::ffi::c_int,
        y: std::ffi::c_int,
        width: std::ffi::c_int,
        height: std::ffi::c_int,
        border_width: std::ffi::c_int,
        depth: std::ffi::c_int,
        visual: *mut std::ffi::c_void,
        root: std::ffi::c_ulong,
        class: std::ffi::c_int,
        bit_gravity: std::ffi::c_int,
        win_gravity: std::ffi::c_int,
        backing_store: std::ffi::c_int,
        backing_planes: std::ffi::c_ulong,
        backing_pixel: std::ffi::c_ulong,
        save_under: std::ffi::c_int,
        colormap: std::ffi::c_ulong,
        map_installed: std::ffi::c_int,
        map_state: std::ffi::c_int,
        all_event_masks: std::ffi::c_long,
        your_event_mask: std::ffi::c_long,
        do_not_propagate_mask: std::ffi::c_long,
        override_redirect: std::ffi::c_int,
        screen: *mut std::ffi::c_void,
    }

    #[repr(C)]
    struct XImageFuncs {
        create_image: *mut std::ffi::c_void,
        destroy_image: Option<unsafe extern "C" fn(*mut XImage) -> std::ffi::c_int>,
        get_pixel: *mut std::ffi::c_void,
        put_pixel: *mut std::ffi::c_void,
        sub_image: *mut std::ffi::c_void,
        add_pixel: *mut std::ffi::c_void,
    }

    #[repr(C)]
    struct XImage {
        width: std::ffi::c_int,
        height: std::ffi::c_int,
        xoffset: std::ffi::c_int,
        format: std::ffi::c_int,
        data: *mut u8,
        byte_order: std::ffi::c_int,
        bitmap_unit: std::ffi::c_int,
        bitmap_bit_order: std::ffi::c_int,
        bitmap_pad: std::ffi::c_int,
        depth: std::ffi::c_int,
        bytes_per_line: std::ffi::c_int,
        bits_per_pixel: std::ffi::c_int,
        red_mask: std::ffi::c_ulong,
        green_mask: std::ffi::c_ulong,
        blue_mask: std::ffi::c_ulong,
        obdata: *mut std::ffi::c_void,
        f: XImageFuncs,
    }

    struct X11Api {
        open_display: unsafe extern "C" fn(*const std::ffi::c_char) -> *mut std::ffi::c_void,
        default_root_window: unsafe extern "C" fn(*mut std::ffi::c_void) -> std::ffi::c_ulong,
        get_window_attributes: unsafe extern "C" fn(
            *mut std::ffi::c_void,
            std::ffi::c_ulong,
            *mut XWindowAttributes,
        ) -> std::ffi::c_int,
        get_image: unsafe extern "C" fn(
            *mut std::ffi::c_void,
            std::ffi::c_ulong,
            std::ffi::c_int,
            std::ffi::c_int,
            std::ffi::c_uint,
            std::ffi::c_uint,
            std::ffi::c_ulong,
            std::ffi::c_int,
        ) -> *mut XImage,
        query_pointer: unsafe extern "C" fn(
            *mut std::ffi::c_void,
            std::ffi::c_ulong,
            *mut std::ffi::c_ulong,
            *mut std::ffi::c_ulong,
            *mut std::ffi::c_int,
            *mut std::ffi::c_int,
            *mut std::ffi::c_int,
            *mut std::ffi::c_int,
            *mut std::ffi::c_uint,
        ) -> std::ffi::c_int,
        close_display: unsafe extern "C" fn(*mut std::ffi::c_void) -> std::ffi::c_int,
        free: unsafe extern "C" fn(*mut std::ffi::c_void) -> std::ffi::c_int,
    }

    fn x11_api() -> Option<&'static X11Api> {
        static API: std::sync::OnceLock<Option<X11Api>> = std::sync::OnceLock::new();
        API.get_or_init(|| unsafe {
            let candidates = [c"libX11.so.6", c"libX11.so"];
            let mut lib = std::ptr::null_mut();
            for name in candidates {
                lib = libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
                if !lib.is_null() {
                    break;
                }
            }
            if lib.is_null() {
                return None;
            }
            Some(X11Api {
                open_display: dl_sym(lib, c"XOpenDisplay")?,
                default_root_window: dl_sym(lib, c"XDefaultRootWindow")?,
                get_window_attributes: dl_sym(lib, c"XGetWindowAttributes")?,
                get_image: dl_sym(lib, c"XGetImage")?,
                query_pointer: dl_sym(lib, c"XQueryPointer")?,
                close_display: dl_sym(lib, c"XCloseDisplay")?,
                free: dl_sym(lib, c"XFree")?,
            })
        })
        .as_ref()
    }

    struct ThreadX11Display(*mut std::ffi::c_void);

    impl Drop for ThreadX11Display {
        fn drop(&mut self) {
            if !self.0.is_null()
                && let Some(api) = x11_api()
            {
                unsafe {
                    let _ = (api.close_display)(self.0);
                }
            }
        }
    }

    fn with_x11_display<R>(f: impl FnOnce(&X11Api, *mut std::ffi::c_void) -> Option<R>) -> Option<R> {
        std::thread_local! {
            static DISPLAY: std::cell::RefCell<Option<ThreadX11Display>> = const { std::cell::RefCell::new(None) };
        }
        let api = x11_api()?;
        DISPLAY.with(|cell| {
            let mut borrow = cell.borrow_mut();
            if borrow.is_none() {
                let dpy = unsafe { (api.open_display)(std::ptr::null()) };
                if !dpy.is_null() {
                    *borrow = Some(ThreadX11Display(dpy));
                }
            }
            let dpy = borrow.as_ref()?.0;
            f(api, dpy)
        })
    }

    fn primary_screen_dimensions() -> (u32, u32) {
        with_x11_display(|api, dpy| unsafe {
            let root = (api.default_root_window)(dpy);
            let mut attrs: XWindowAttributes = std::mem::zeroed();
            if (api.get_window_attributes)(dpy, root, &mut attrs) != 0 && attrs.width > 0 && attrs.height > 0
            {
                Some((attrs.width as u32, attrs.height as u32))
            } else {
                None
            }
        })
        .unwrap_or((1920, 1080))
    }

    fn draw_cursor_pointer(bgra: &mut [u8], dst_w: u32, dst_h: u32, cx: i32, cy: i32) {
        // 12x16 arrow cursor bitmap (1 = black border, 2 = white fill).
        const CURSOR: [[u8; 10]; 14] = [
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [1, 1, 0, 0, 0, 0, 0, 0, 0, 0],
            [1, 2, 1, 0, 0, 0, 0, 0, 0, 0],
            [1, 2, 2, 1, 0, 0, 0, 0, 0, 0],
            [1, 2, 2, 2, 1, 0, 0, 0, 0, 0],
            [1, 2, 2, 2, 2, 1, 0, 0, 0, 0],
            [1, 2, 2, 2, 2, 2, 1, 0, 0, 0],
            [1, 2, 2, 2, 2, 2, 2, 1, 0, 0],
            [1, 2, 2, 2, 2, 2, 2, 2, 1, 0],
            [1, 2, 2, 2, 2, 1, 1, 1, 1, 1],
            [1, 2, 2, 1, 2, 1, 0, 0, 0, 0],
            [1, 2, 1, 0, 1, 2, 1, 0, 0, 0],
            [1, 1, 0, 0, 1, 2, 1, 0, 0, 0],
            [0, 0, 0, 0, 0, 1, 1, 0, 0, 0],
        ];
        for (dy, row) in CURSOR.iter().enumerate() {
            let py = cy + dy as i32;
            if py < 0 || py >= dst_h as i32 {
                continue;
            }
            for (dx, &cell) in row.iter().enumerate() {
                if cell == 0 {
                    continue;
                }
                let px = cx + dx as i32;
                if px < 0 || px >= dst_w as i32 {
                    continue;
                }
                let idx = ((py as usize) * (dst_w as usize) + (px as usize)) * 4;
                let val = if cell == 2 { 0xFF } else { 0x00 };
                bgra[idx] = val;
                bgra[idx + 1] = val;
                bgra[idx + 2] = val;
                bgra[idx + 3] = 0xFF;
            }
        }
    }

    fn has_executable(name: &str) -> bool {
        let Some(path_var) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&path_var).any(|dir| dir.join(name).is_file())
    }

    /// Whether gst-launch and its PipeWire source are installed (gstreamer1.0-tools
    /// and gstreamer1.0-pipewire on Debian and Ubuntu), which Wayland capture uses.
    pub(crate) fn pipewire_capture_installed() -> bool {
        has_executable("gst-launch-1.0")
            && std::process::Command::new("gst-inspect-1.0")
                .args(["--exists", "pipewiresrc"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
    }

    fn is_wayland_screencast_available() -> bool {
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return false;
        }
        static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *AVAILABLE.get_or_init(|| {
            if !pipewire_capture_installed() {
                return false;
            }
            let Ok(conn) = zbus::blocking::Connection::session() else {
                return false;
            };
            let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else {
                return false;
            };
            "org.freedesktop.portal.Desktop"
                .try_into()
                .ok()
                .and_then(|name| dbus.name_has_owner(name).ok())
                .unwrap_or(false)
        })
    }

    struct WaylandScreenCaster {
        /// The session lasts as long as this connection.
        conn: zbus::blocking::Connection,
        /// The session's object path, for opening more PipeWire connections to it.
        session: String,
        /// The PipeWire connection the portal opened for us; the stream is
        /// only readable through it.
        remote: std::os::fd::OwnedFd,
        node_id: u32,
        started: Instant,
        src_w: u32,
        src_h: u32,
        dst_w: u32,
        dst_h: u32,
        child: Option<std::process::Child>,
        latest_frame: Arc<Mutex<Option<Vec<u8>>>>,
    }

    impl Drop for WaylandScreenCaster {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    /// A started ScreenCast session: the connection it lives on (it ends with
    /// it) and the PipeWire stream of the monitor the user chose.
    struct Negotiated {
        conn: zbus::blocking::Connection,
        session: String,
        remote: std::os::fd::OwnedFd,
        node_id: u32,
        src_w: u32,
        src_h: u32,
    }

    fn portal_failed(error: impl std::fmt::Display) -> PcCaptureError {
        PcCaptureError::Failed(format!("screen-sharing portal: {error}"))
    }

    /// Marks the portal's dialog as answered when it goes, on every way out.
    struct Answered(Arc<AtomicBool>);

    impl Drop for Answered {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    fn wait_portal_response(
        iter: &mut zbus::blocking::MessageIterator,
        expected_path: &str,
    ) -> Result<std::collections::HashMap<String, zbus::zvariant::OwnedValue>, PcCaptureError> {
        for msg in iter.by_ref() {
            let msg = msg.map_err(portal_failed)?;
            let hdr = msg.header();
            if hdr.interface().map(|i| i.as_str()) == Some("org.freedesktop.portal.Request")
                && hdr.member().map(|m| m.as_str()) == Some("Response")
                && hdr.path().map(|p| p.as_str()) == Some(expected_path)
            {
                let (code, results): (u32, std::collections::HashMap<String, zbus::zvariant::OwnedValue>) =
                    msg.body().deserialize().map_err(portal_failed)?;
                return match code {
                    0 => Ok(results),
                    // The user dismissed the dialog.
                    1 => Err(PcCaptureError::Declined),
                    other => Err(portal_failed(format!("answered {other}"))),
                };
            }
        }
        Err(portal_failed("the session bus went away"))
    }

    /// Opens another connection to a session's PipeWire streams.
    fn open_pipewire_remote(
        conn: &zbus::blocking::Connection,
        session: &str,
    ) -> Result<std::os::fd::OwnedFd, PcCaptureError> {
        use std::collections::HashMap;
        use zbus::zvariant::{ObjectPath, Value};

        let session = ObjectPath::try_from(session).map_err(portal_failed)?;
        let empty_opts: HashMap<&str, Value<'_>> = HashMap::new();
        let reply = conn
            .call_method(
                Some("org.freedesktop.portal.Desktop"),
                "/org/freedesktop/portal/desktop",
                Some("org.freedesktop.portal.ScreenCast"),
                "OpenPipeWireRemote",
                &(&session, &empty_opts),
            )
            .map_err(portal_failed)?;
        let remote: zbus::zvariant::OwnedFd = reply.body().deserialize().map_err(portal_failed)?;
        Ok(remote.into())
    }

    impl WaylandScreenCaster {
        /// Opens a ScreenCast session on one monitor: `cursor_mode` 1 leaves the
        /// pointer out, 2 draws it in. `prompt` hears when the portal likely asks
        /// the user: it has no earlier choice to restore, or doesn't answer at once.
        fn negotiate_portal(
            cursor_mode: u32,
            prompt: Option<CapturePrompt>,
        ) -> Result<Negotiated, PcCaptureError> {
            use std::collections::HashMap;
            use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

            let conn = zbus::blocking::Connection::session().map_err(portal_failed)?;
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface("org.freedesktop.portal.Request")
                .and_then(|b| b.member("Response"))
                .map(|b| b.build())
                .map_err(portal_failed)?;
            let dbus = zbus::blocking::fdo::DBusProxy::new(&conn).map_err(portal_failed)?;
            dbus.add_match_rule(rule).map_err(portal_failed)?;
            let mut iter = zbus::blocking::MessageIterator::from(&conn);

            let pid = std::process::id();

            // 1. CreateSession
            let mut create_opts: HashMap<&str, Value<'_>> = HashMap::new();
            let req_tok_1 = format!("g7link_sc_req1_{pid}");
            let sess_tok = format!("g7link_sc_sess_{pid}");
            create_opts.insert("handle_token", Value::from(req_tok_1.as_str()));
            create_opts.insert("session_handle_token", Value::from(sess_tok.as_str()));

            let reply = conn
                .call_method(
                    Some("org.freedesktop.portal.Desktop"),
                    "/org/freedesktop/portal/desktop",
                    Some("org.freedesktop.portal.ScreenCast"),
                    "CreateSession",
                    &(&create_opts,),
                )
                .map_err(portal_failed)?;
            let req_path_1: OwnedObjectPath = reply.body().deserialize().map_err(portal_failed)?;
            let create_res = wait_portal_response(&mut iter, req_path_1.as_str())?;
            let session_path_str = create_res.get("session_handle").and_then(|v| {
                String::try_from(v.clone())
                    .ok()
                    .or_else(|| OwnedObjectPath::try_from(v.clone()).ok().map(|p| p.to_string()))
            });
            let session_path_str = session_path_str.ok_or_else(|| portal_failed("no session"))?;
            let session_obj = ObjectPath::try_from(session_path_str.as_str()).map_err(portal_failed)?;

            // 2. SelectSources (types = 1 MONITOR, one of them)
            let mut select_opts: HashMap<&str, Value<'_>> = HashMap::new();
            let req_tok_2 = format!("g7link_sc_req2_{pid}");
            select_opts.insert("handle_token", Value::from(req_tok_2.as_str()));
            select_opts.insert("types", Value::from(1u32));
            select_opts.insert("multiple", Value::from(false));
            select_opts.insert("cursor_mode", Value::from(cursor_mode));
            // Ask once until the user takes it back: the portal hands back a token
            // that restores the same choice without a dialog, also after a restart.
            select_opts.insert("persist_mode", Value::from(2u32));
            let restore = read_restore_token(&restore_token_path());
            if let Some(token) = restore.as_deref() {
                select_opts.insert("restore_token", Value::from(token));
            }

            let reply = conn
                .call_method(
                    Some("org.freedesktop.portal.Desktop"),
                    "/org/freedesktop/portal/desktop",
                    Some("org.freedesktop.portal.ScreenCast"),
                    "SelectSources",
                    &(&session_obj, &select_opts),
                )
                .map_err(portal_failed)?;
            let req_path_2: OwnedObjectPath = reply.body().deserialize().map_err(portal_failed)?;
            wait_portal_response(&mut iter, req_path_2.as_str())?;

            // 3. Start: the portal asks the user now, unless it restores an earlier
            // choice, which it does at once. A slow answer means it asks after all.
            let answered = Answered(Arc::new(AtomicBool::new(false)));
            if let Some(prompt) = prompt {
                if restore.is_none() {
                    prompt();
                } else {
                    let answered = Arc::clone(&answered.0);
                    let ask = move || {
                        std::thread::sleep(PROMPT_AFTER);
                        if !answered.load(Ordering::SeqCst) {
                            prompt();
                        }
                    };
                    let _ = std::thread::Builder::new().name("g7link-screencast-ask".into()).spawn(ask);
                }
            }
            let mut start_opts: HashMap<&str, Value<'_>> = HashMap::new();
            let req_tok_3 = format!("g7link_sc_req3_{pid}");
            start_opts.insert("handle_token", Value::from(req_tok_3.as_str()));

            let reply = conn
                .call_method(
                    Some("org.freedesktop.portal.Desktop"),
                    "/org/freedesktop/portal/desktop",
                    Some("org.freedesktop.portal.ScreenCast"),
                    "Start",
                    &(&session_obj, "", &start_opts),
                )
                .map_err(portal_failed)?;
            let req_path_3: OwnedObjectPath = reply.body().deserialize().map_err(portal_failed)?;
            let start_res = wait_portal_response(&mut iter, req_path_3.as_str())?;
            drop(answered);
            if let Some(token) = start_res.get("restore_token").and_then(|v| String::try_from(v.clone()).ok())
            {
                // Each token works once, and this is the one for next time.
                if let Err(error) = save_restore_token(&restore_token_path(), &token) {
                    tracing::warn!(%error, "can't save the screen choice");
                }
            }

            let streams_val = start_res.get("streams").ok_or_else(|| portal_failed("no streams"))?;
            let streams: Vec<(u32, HashMap<String, OwnedValue>)> =
                streams_val.clone().try_into().map_err(portal_failed)?;
            let (node_id, props) = streams.into_iter().next().ok_or_else(|| portal_failed("no streams"))?;
            let (src_w, src_h) = props
                .get("size")
                .and_then(|v| <(i32, i32)>::try_from(v.clone()).ok())
                .filter(|&(w, h)| w > 0 && h > 0)
                .map(|(w, h)| (w as u32, h as u32))
                .unwrap_or((1920, 1080));

            // The stream is only readable through the PipeWire connection
            // the portal opens for this session.
            let remote = open_pipewire_remote(&conn, &session_path_str)?;

            Ok(Negotiated { conn, session: session_path_str, remote, node_id, src_w, src_h })
        }

        /// The fd number the portal's PipeWire connection gets in the capture process.
        const REMOTE_FD: i32 = 3;

        /// Starts gst-launch reading the stream through `remote` and writing
        /// raw BGRx frames of `dst_w` x `dst_h` to its stdout.
        #[allow(clippy::type_complexity)]
        fn spawn_pipeline(
            remote: &std::os::fd::OwnedFd,
            node_id: u32,
            dst_w: u32,
            dst_h: u32,
        ) -> Option<(std::process::Child, Arc<Mutex<Option<Vec<u8>>>>)> {
            use std::io::Read;
            use std::os::fd::AsRawFd;
            use std::os::unix::process::CommandExt;
            use std::process::{Command, Stdio};

            let raw = remote.as_raw_fd();
            let mut cmd = Command::new("gst-launch-1.0");
            cmd.args(pipeline_args(node_id, dst_w, dst_h))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            // SAFETY: runs in the child between fork and exec and only calls
            // async-signal-safe functions (dup2, fcntl) on descriptors we own.
            unsafe {
                cmd.pre_exec(move || {
                    if raw == Self::REMOTE_FD {
                        // Already in place: only let it survive exec.
                        if libc::fcntl(raw, libc::F_SETFD, 0) < 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                    } else if libc::dup2(raw, Self::REMOTE_FD) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let mut child = cmd.spawn().ok()?;

            let mut stdout = child.stdout.take()?;
            let latest_frame: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
            let frame_slot = Arc::clone(&latest_frame);
            let frame_bytes = (dst_w as usize) * (dst_h as usize) * 4;

            std::thread::Builder::new()
                .name("wayland-screencast-reader".into())
                .spawn(move || {
                    let mut buf = vec![0u8; frame_bytes];
                    while stdout.read_exact(&mut buf).is_ok() {
                        for chunk in buf.as_chunks_mut::<4>().0 {
                            chunk[3] = 0xFF;
                        }
                        *frame_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(buf.clone());
                    }
                })
                .ok()?;

            Some((child, latest_frame))
        }

        fn start(dst_w: u32, dst_h: u32) -> Option<Self> {
            Self::open(2, None, |_, _| (dst_w, dst_h))
                .inspect_err(|error| tracing::debug!(%error, "can't share the screen"))
                .ok()
        }

        /// Opens a session and starts reading its stream at the size `dims`
        /// picks for the monitor's.
        fn open(
            cursor_mode: u32,
            prompt: Option<CapturePrompt>,
            dims: impl FnOnce(u32, u32) -> (u32, u32),
        ) -> Result<Self, PcCaptureError> {
            let n = Self::negotiate_portal(cursor_mode, prompt)?;
            let (dst_w, dst_h) = dims(n.src_w, n.src_h);
            let pipeline = Self::spawn_pipeline(&n.remote, n.node_id, dst_w, dst_h);
            let (child, latest_frame) = pipeline.ok_or_else(|| portal_failed("can't read the screen"))?;
            Ok(Self {
                conn: n.conn,
                session: n.session,
                remote: n.remote,
                node_id: n.node_id,
                started: Instant::now(),
                src_w: n.src_w,
                src_h: n.src_h,
                dst_w,
                dst_h,
                child: Some(child),
                latest_frame,
            })
        }

        /// Whether the capture process is still running.
        fn alive(&mut self) -> bool {
            self.child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)))
        }

        fn capture(&mut self, dst_w: u32, dst_h: u32, bgra: &mut Vec<u8>) -> Option<(u32, u32)> {
            if (self.dst_w, self.dst_h) != (dst_w, dst_h) {
                if let Some(mut old) = self.child.take() {
                    let _ = old.kill();
                    let _ = old.wait();
                }
                let (child, latest_frame) = Self::spawn_pipeline(&self.remote, self.node_id, dst_w, dst_h)?;
                self.started = Instant::now();
                self.dst_w = dst_w;
                self.dst_h = dst_h;
                self.child = Some(child);
                self.latest_frame = latest_frame;
            }

            // The first frame can take a while (the pipeline negotiates
            // formats); after that a frame is late only for a moment.
            let wait = if self.started.elapsed() < Duration::from_secs(3) {
                Duration::from_secs(3).saturating_sub(self.started.elapsed())
            } else {
                Duration::from_millis(250)
            };
            // Its last frame would otherwise show as a live screen for good.
            if !self.alive() {
                return None;
            }
            let deadline = Instant::now() + wait;
            loop {
                if let Some(frame) = self.latest_frame.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                    bgra.clear();
                    bgra.extend_from_slice(frame);
                    return Some((self.src_w, self.src_h));
                }
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    static WAYLAND_CASTER: Mutex<Option<WaylandScreenCaster>> = Mutex::new(None);

    /// Where the ScreenCast portal's restore token is kept, so later sessions
    /// reuse the screen the user chose instead of asking again.
    fn restore_token_path() -> std::path::PathBuf {
        core_host::host().data_dir.join("screencast-restore-token")
    }

    /// A token that's gone or unusable just means the portal asks again.
    fn read_restore_token(path: &std::path::Path) -> Option<String> {
        let token = std::fs::read_to_string(path).ok()?;
        let token = token.trim();
        (!token.is_empty() && token.len() <= 256).then(|| token.to_owned())
    }

    /// Only this user may read it: it lets this app see the screen without asking.
    fn save_restore_token(path: &std::path::Path, token: &str) -> std::io::Result<()> {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let tmp = path.with_extension("tmp");
        let mut file =
            std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(&tmp)?;
        file.write_all(token.as_bytes())?;
        drop(file);
        std::fs::rename(&tmp, path)
    }

    /// gst-launch arguments: read stream `node_id` through the portal's
    /// PipeWire connection (fd 3) and write raw BGRx frames to stdout.
    pub(crate) fn pipeline_args(node_id: u32, dst_w: u32, dst_h: u32) -> Vec<String> {
        [
            "-q".to_owned(),
            "pipewiresrc".to_owned(),
            format!("fd={}", WaylandScreenCaster::REMOTE_FD),
            format!("path={node_id}"),
            "do-timestamp=true".to_owned(),
            "!".to_owned(),
            "videoconvert".to_owned(),
            "!".to_owned(),
            "videoscale".to_owned(),
            "!".to_owned(),
            format!("video/x-raw,format=BGRx,width={dst_w},height={dst_h}"),
            "!".to_owned(),
            "fdsink".to_owned(),
            "fd=1".to_owned(),
        ]
        .into()
    }

    fn capture_screen_wayland(dst_w: u32, dst_h: u32, bgra: &mut Vec<u8>) -> Option<(u32, u32)> {
        if !is_wayland_screencast_available() {
            return None;
        }
        let mut guard = WAYLAND_CASTER.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = WaylandScreenCaster::start(dst_w, dst_h);
        }
        let caster = guard.as_mut()?;
        let res = caster.capture(dst_w, dst_h, bgra);
        // A late frame is skipped; only a stopped capture ends the session
        // (starting a new one would ask the user again).
        if res.is_none() && !caster.alive() {
            *guard = None;
        }
        res
    }

    fn stop_wayland_screencast() {
        *WAYLAND_CASTER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Ends the screencast when no phone sees the screen any more.
    pub(crate) fn release_screencast() {
        if !is_active() {
            stop_wayland_screencast();
        }
    }

    /// How long after asking for the screen the portal is taken to show its dialog.
    const PROMPT_AFTER: Duration = Duration::from_millis(1500);
    /// How long a screenshot waits for the screen's first picture.
    const STILL_WAIT: Duration = Duration::from_secs(5);
    /// The first picture of a new stream can be half drawn: a screenshot is the
    /// newest one this long after it.
    const STILL_SETTLE: Duration = Duration::from_millis(200);
    /// The largest screen a screenshot takes whole, in pixels.
    const MAX_STILL_PIXELS: u64 = 8192 * 8192;
    /// The longest side of a recording.
    pub(crate) const RECORD_MAX_EDGE: u32 = 1920;

    /// The screen at its own size, for a screenshot: through the screencast a
    /// phone or a recording already uses (pointer included), or through a
    /// session of its own without the pointer, which may ask the user first.
    pub(crate) fn grab_still(prompt: CapturePrompt) -> Result<crate::sys::image::Bitmap, PcCaptureError> {
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return grab_still_x11();
        }
        if !is_wayland_screencast_available() {
            return Err(PcCaptureError::Unsupported);
        }
        let running = WAYLAND_CASTER
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|c| (c.conn.clone(), c.session.clone(), c.node_id, c.src_w, c.src_h));
        match running {
            Some((conn, session, node_id, src_w, src_h)) => {
                let remote = open_pipewire_remote(&conn, &session)?;
                one_frame(&remote, node_id, src_w, src_h)
            }
            None => {
                // The session lasts while its connection does: until the picture is in.
                let session = WaylandScreenCaster::negotiate_portal(1, Some(prompt))?;
                one_frame(&session.remote, session.node_id, session.src_w, session.src_h)
            }
        }
    }

    /// One picture of stream `node_id` at `width` x `height`.
    fn one_frame(
        remote: &std::os::fd::OwnedFd,
        node_id: u32,
        width: u32,
        height: u32,
    ) -> Result<crate::sys::image::Bitmap, PcCaptureError> {
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_STILL_PIXELS {
            return Err(portal_failed(format!("a {width}x{height} screen")));
        }
        let pipeline = WaylandScreenCaster::spawn_pipeline(remote, node_id, width, height);
        let (mut child, latest) = pipeline.ok_or_else(|| portal_failed("can't read the screen"))?;
        let deadline = Instant::now() + STILL_WAIT;
        let mut first_at: Option<Instant> = None;
        let frame = loop {
            let ready = latest.lock().unwrap_or_else(|e| e.into_inner()).is_some();
            if ready && first_at.is_none() {
                first_at = Some(Instant::now());
            }
            let settled = first_at.is_some_and(|at| at.elapsed() >= STILL_SETTLE);
            let ended = !matches!(child.try_wait(), Ok(None));
            if settled || ended || Instant::now() >= deadline {
                break latest.lock().unwrap_or_else(|e| e.into_inner()).take();
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let _ = child.kill();
        let _ = child.wait();
        let bgra = frame.ok_or_else(|| portal_failed("no picture came"))?;
        Ok(crate::sys::image::Bitmap { width, height, bgra })
    }

    fn grab_still_x11() -> Result<crate::sys::image::Bitmap, PcCaptureError> {
        if std::env::var_os("DISPLAY").is_none() || x11_api().is_none() {
            return Err(PcCaptureError::Unsupported);
        }
        let (width, height) = primary_screen_dimensions();
        let whole = crate::sys::browser::Rect { left: 0, top: 0, right: width as i32, bottom: height as i32 };
        let mut bgra = Vec::new();
        // A region of the screen comes without the pointer.
        if capture_screen(Some(whole), width, height, &mut bgra).is_none() {
            return Err(PcCaptureError::Failed("can't read the screen".into()));
        }
        Ok(crate::sys::image::Bitmap { width, height, bgra })
    }

    /// Where a recording gets the screen from, pointer included: the
    /// screencast a phone sees when there is one, or one opened for it.
    pub(crate) enum ScreenFeed {
        Wayland,
        X11,
    }

    /// A recording's next picture.
    pub(crate) enum FeedFrame {
        Got,
        NotYet,
        /// The screen can't be read any more (the screencast stopped).
        Gone,
    }

    impl ScreenFeed {
        /// Opens the screen, asking the user first when the portal does
        /// (`prompt`). Returns the feed and the screen's size.
        pub(crate) fn open(prompt: CapturePrompt) -> Result<(ScreenFeed, u32, u32), PcCaptureError> {
            if std::env::var_os("WAYLAND_DISPLAY").is_none() {
                if std::env::var_os("DISPLAY").is_none() || x11_api().is_none() {
                    return Err(PcCaptureError::Unsupported);
                }
                let (width, height) = primary_screen_dimensions();
                return Ok((ScreenFeed::X11, width, height));
            }
            if !is_wayland_screencast_available() {
                return Err(PcCaptureError::Unsupported);
            }
            if let Some(caster) = WAYLAND_CASTER.lock().unwrap_or_else(|e| e.into_inner()).as_mut()
                && caster.alive()
            {
                return Ok((ScreenFeed::Wayland, caster.src_w, caster.src_h));
            }
            // Not under the lock: the portal may wait for the user.
            let dims = |w: u32, h: u32| target_dimensions(w, h, RECORD_MAX_EDGE);
            let caster = WaylandScreenCaster::open(2, Some(prompt), dims)?;
            let (width, height) = (caster.src_w, caster.src_h);
            let mut guard = WAYLAND_CASTER.lock().unwrap_or_else(|e| e.into_inner());
            // A phone may have started seeing the screen meanwhile: its screencast stays.
            if guard.as_mut().is_none_or(|c| !c.alive()) {
                *guard = Some(caster);
            }
            Ok((ScreenFeed::Wayland, width, height))
        }

        /// The newest picture of the screen, at `width` x `height`.
        pub(crate) fn frame(&self, width: u32, height: u32, bgra: &mut Vec<u8>) -> FeedFrame {
            match self {
                ScreenFeed::X11 => {
                    if capture_screen(None, width, height, bgra).is_some() {
                        FeedFrame::Got
                    } else {
                        FeedFrame::NotYet
                    }
                }
                ScreenFeed::Wayland => {
                    let (slot, src_w, src_h) = {
                        let mut guard = WAYLAND_CASTER.lock().unwrap_or_else(|e| e.into_inner());
                        match guard.as_mut() {
                            Some(caster) if caster.alive() => {
                                (Arc::clone(&caster.latest_frame), caster.dst_w, caster.dst_h)
                            }
                            _ => return FeedFrame::Gone,
                        }
                    };
                    let latest = slot.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(picture) = latest.as_ref() else { return FeedFrame::NotYet };
                    if scale_bgra(picture, src_w, src_h, width, height, bgra) {
                        FeedFrame::Got
                    } else {
                        FeedFrame::NotYet
                    }
                }
            }
        }
    }

    /// Scales a `src_w` x `src_h` BGRA picture to `dst_w` x `dst_h` (nearest
    /// pixel) into `dst`. False when `src` is shorter than its size says.
    pub(crate) fn scale_bgra(
        src: &[u8],
        src_w: u32,
        src_h: u32,
        dst_w: u32,
        dst_h: u32,
        dst: &mut Vec<u8>,
    ) -> bool {
        let (sw, sh, dw, dh) = (src_w as usize, src_h as usize, dst_w as usize, dst_h as usize);
        if sw == 0 || sh == 0 || dw == 0 || dh == 0 || src.len() < sw * sh * 4 {
            return false;
        }
        if (sw, sh) == (dw, dh) {
            dst.clear();
            dst.extend_from_slice(&src[..sw * sh * 4]);
            return true;
        }
        dst.resize(dw * dh * 4, 0);
        for (y, row) in dst.chunks_exact_mut(dw * 4).enumerate() {
            let sy = y * sh / dh;
            let src_row = &src[sy * sw * 4..(sy + 1) * sw * 4];
            for (x, px) in row.chunks_exact_mut(4).enumerate() {
                let sx = x * sw / dw;
                px.copy_from_slice(&src_row[sx * 4..sx * 4 + 4]);
            }
        }
        true
    }

    fn capture_screen(
        region: Option<crate::sys::browser::Rect>,
        dst_w: u32,
        dst_h: u32,
        bgra: &mut Vec<u8>,
    ) -> Option<(u32, u32)> {
        if region.is_none()
            && std::env::var_os("WAYLAND_DISPLAY").is_some()
            && let Some(dims) = capture_screen_wayland(dst_w, dst_h, bgra)
        {
            return Some(dims);
        }
        with_x11_display(|api, dpy| unsafe {
            let root = (api.default_root_window)(dpy);
            let mut attrs: XWindowAttributes = std::mem::zeroed();
            if (api.get_window_attributes)(dpy, root, &mut attrs) == 0
                || attrs.width <= 0
                || attrs.height <= 0
            {
                return None;
            }
            let full_w = attrs.width;
            let full_h = attrs.height;
            let (src_x, src_y, src_w, src_h) = match region {
                Some(r) => {
                    let x0 = r.left.clamp(0, full_w - 1);
                    let y0 = r.top.clamp(0, full_h - 1);
                    let x1 = r.right.clamp(x0 + 1, full_w);
                    let y1 = r.bottom.clamp(y0 + 1, full_h);
                    (x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
                }
                None => (0, 0, full_w as u32, full_h as u32),
            };

            let img = (api.get_image)(
                dpy,
                root,
                src_x,
                src_y,
                src_w,
                src_h,
                !0 as std::ffi::c_ulong, // AllPlanes
                2,                       // ZPixmap
            );
            if img.is_null() {
                return None;
            }
            if (*img).data.is_null() {
                if let Some(destroy) = (*img).f.destroy_image {
                    let _ = destroy(img);
                } else {
                    let _ = (api.free)(img.cast());
                }
                return None;
            }

            let bytes_per_line = (*img).bytes_per_line.max(0) as usize;
            let bpp = ((*img).bits_per_pixel.max(24) as usize) / 8;
            let needed = (dst_w as usize) * (dst_h as usize) * 4;
            bgra.resize(needed, 0);

            for y in 0..dst_h {
                let sy =
                    ((u64::from(y) * u64::from(src_h)) / u64::from(dst_h)).min(u64::from(src_h - 1)) as usize;
                let src_row = (*img).data.add(sy * bytes_per_line);
                let dst_row_offset = (y as usize) * (dst_w as usize) * 4;
                for x in 0..dst_w {
                    let sx = ((u64::from(x) * u64::from(src_w)) / u64::from(dst_w)).min(u64::from(src_w - 1))
                        as usize;
                    let px = src_row.add(sx * bpp);
                    let dst_idx = dst_row_offset + (x as usize) * 4;
                    bgra[dst_idx] = *px;
                    bgra[dst_idx + 1] = *px.add(1);
                    bgra[dst_idx + 2] = *px.add(2);
                    bgra[dst_idx + 3] = 0xFF;
                }
            }

            let mut root_ret: std::ffi::c_ulong = 0;
            let mut child_ret: std::ffi::c_ulong = 0;
            let mut root_x: std::ffi::c_int = 0;
            let mut root_y: std::ffi::c_int = 0;
            let mut win_x: std::ffi::c_int = 0;
            let mut win_y: std::ffi::c_int = 0;
            let mut mask_ret: std::ffi::c_uint = 0;
            if region.is_none()
                && (api.query_pointer)(
                    dpy,
                    root,
                    &mut root_ret,
                    &mut child_ret,
                    &mut root_x,
                    &mut root_y,
                    &mut win_x,
                    &mut win_y,
                    &mut mask_ret,
                ) != 0
                && root_x >= 0
                && root_y >= 0
            {
                let cx = ((root_x as i64) * (dst_w as i64) / (src_w as i64)) as i32;
                let cy = ((root_y as i64) * (dst_h as i64) / (src_h as i64)) as i32;
                draw_cursor_pointer(bgra, dst_w, dst_h, cx, cy);
            }

            if let Some(destroy) = (*img).f.destroy_image {
                let _ = destroy(img);
            } else {
                libc::free((*img).data.cast());
                let _ = (api.free)(img.cast());
            }

            Some((src_w, src_h))
        })
    }

    fn run_capture_loop(
        peer: DeviceId,
        options: MirrorStart,
        stop: Arc<AtomicBool>,
        force_keyframe: Arc<AtomicBool>,
    ) {
        let Some(node) = core_host::node() else {
            cleanup_session(&stop);
            return;
        };
        let runtime = &core_host::host().runtime;
        let Ok(sender) = runtime.block_on(node.mirror_open(peer)) else {
            tracing::warn!(%peer, "couldn't open PC mirror stream to phone");
            cleanup_session(&stop);
            return;
        };

        // Give the phone's PcMirrorView TextureView and MediaCodec a brief moment to
        // mount before sending the initial MirrorConfig and IDR keyframe.
        std::thread::sleep(Duration::from_millis(120));

        let fps = options.fps.clamp(10, 60);
        let max_bitrate = options.bitrate.clamp(500_000, 20_000_000);
        // Lowered when the link can't keep up (frames dropped to hold the delay down),
        // raised back once it has been clean for a while.
        let mut bitrate = max_bitrate;
        let frame_interval = Duration::from_micros(1_000_000 / u64::from(fps));
        let dur_hns = (10_000_000 / u64::from(fps)) as i64;

        let mut codec = if node.mirror_codecs(peer).is_ok_and(|codecs| codecs.iter().any(|c| c == "h265")) {
            "h265".to_owned()
        } else {
            "h264".to_owned()
        };

        let mut bgra = Vec::new();
        let mut nv12 = Vec::new();
        let mut stream_dims: Option<(u32, u32)> = None;
        let mut encoder: Option<ScreenEncoder> = None;
        let started = Instant::now();
        let mut last_reinit = Instant::now() - Duration::from_secs(5);
        let mut last_drop: Option<Instant> = None;
        let mut last_rate_change = Instant::now();
        let mut link_measured = false;
        let mut stats = Stats::default();
        let mut stats_since = Instant::now();
        // Play on phone: the player's picture instead of the screen, and the
        // PC's sound to that phone while it watches. The picture's place comes
        // in physical pixels; so does this thread's capture.
        if options.watch.is_some() {
            sys::browser::use_physical_pixels();
        }
        let watched = options.watch.as_deref().map(crate::play_on_phone::Watched::follow);
        let mut window_capture: Option<sys::window_capture::WindowCapture> = None;
        let _sound = watched.is_some().then(|| SoundToPhone::start(peer));
        if watched.is_some() {
            tracing::info!(%peer, fps, bitrate, "streaming a player to the phone");
        } else {
            tracing::info!(%peer, fps, bitrate, "sharing this PC's screen");
        }

        while !stop.load(Ordering::Relaxed) {
            let tick_start = Instant::now();

            let place = watched.as_ref().and_then(crate::play_on_phone::Watched::place);
            if watched.is_some() && place.is_none() && started.elapsed() < FIND_WINDOW {
                // Not found yet: nothing to show.
                std::thread::sleep(frame_interval);
                continue;
            }
            if watched.is_some() && place.is_none() {
                // The player's window can't be found. Not the screen instead:
                // that would show the phone whatever else is on it.
                tracing::info!(%peer, "can't find the player's window; stopping");
                crate::bridge::app::show_message(crate::i18n::tr!(
                    "PcMirror",
                    "Can't find the player's window. Play the video on this PC again, then turn on Play on phone."
                ));
                break;
            }
            if let Some(place) = place {
                let r = place.on_screen;
                let dims =
                    target_dimensions((r.right - r.left) as u32, (r.bottom - r.top) as u32, options.max_size);
                if stream_dims != Some(dims) {
                    // A new size (the window moved to full screen, say): a new stream config.
                    stream_dims = Some(dims);
                    encoder = None;
                }
                // The window's own pixels, whatever is in front of it; the
                // screen there when the window can't be captured.
                if window_capture.as_ref().is_none_or(|c| c.window() != place.window()) {
                    window_capture = sys::window_capture::WindowCapture::new(place.window())
                        .inspect_err(
                            |e| tracing::info!(error = %e, "can't capture the window; capturing the screen"),
                        )
                        .ok();
                }
            }

            let (dst_w, dst_h) = match stream_dims {
                Some(dims) => dims,
                None => {
                    let (src_w, src_h) = primary_screen_dimensions();
                    let dims = target_dimensions(src_w, src_h, options.max_size);
                    stream_dims = Some(dims);
                    dims
                }
            };

            // A keyframe on request comes from the running encoder. Rebuilding it (a new
            // stream config plus a full keyframe) is the fallback for encoders that can't.
            let requested_kf = force_keyframe.swap(false, Ordering::SeqCst);
            let keyframe_in_place =
                requested_kf && encoder.as_ref().is_some_and(ScreenEncoder::request_keyframe);
            let need_reinit = encoder.is_none()
                || (requested_kf
                    && !keyframe_in_place
                    && last_reinit.elapsed() >= Duration::from_millis(1400));

            if need_reinit {
                match new_pc_encoder_with_fallback(&mut codec, dst_w, dst_h, fps, bitrate) {
                    Ok(enc) => {
                        let cfg = MirrorConfig {
                            codec: codec.clone(),
                            width: dst_w,
                            height: dst_h,
                            session: g7link_core::MIRROR_SCREEN,
                        };
                        if matches!(sender.send(PacketKind::Config, 0, cfg.to_cbor()), MirrorSend::Closed) {
                            break;
                        }
                        encoder = Some(enc);
                        last_reinit = Instant::now();
                        stats.rebuilds += 1;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, codec, "PC mirror encoder init failed");
                        break;
                    }
                }
            }

            // The link is held: the phone hasn't said it got what was sent. A picture made
            // now would only be dropped, and a keyframe made for nothing costs the most. One
            // asked for above stays asked for, and comes with the next picture that can go.
            let held = sender.is_backed_up();
            if held {
                stats.dropped += 1;
                last_drop = Some(Instant::now());
            }
            let captured = if held {
                false
            } else {
                match (place, window_capture.as_mut()) {
                    (Some(place), Some(capture)) => capture.capture(place.in_window, dst_w, dst_h, &mut bgra),
                    (Some(place), None) => {
                        capture_screen(Some(place.on_screen), dst_w, dst_h, &mut bgra).is_some()
                    }
                    (None, _) => capture_screen(None, dst_w, dst_h, &mut bgra).is_some(),
                }
            };
            if captured && let Some(enc) = encoder.as_mut() {
                bgra_to_nv12(&bgra, dst_w as usize, dst_h as usize, &mut nv12);
                let time_us = started.elapsed().as_micros() as u64;
                let hns = (time_us * 10) as i64;
                match enc.encode(&nv12, hns, dur_hns, &stop) {
                    Ok(packets) => {
                        let mut send_failed = false;
                        for (is_keyframe, bytes) in packets {
                            let kind = if is_keyframe { PacketKind::Keyframe } else { PacketKind::Frame };
                            let len = bytes.len();
                            match sender.send(kind, time_us, bytes) {
                                MirrorSend::Queued => {
                                    stats.frames += 1;
                                    stats.bytes += len as u64;
                                    if is_keyframe {
                                        stats.keyframes += 1;
                                    }
                                }
                                MirrorSend::Dropped => stats.dropped += 1,
                                MirrorSend::NeedKeyframe => {
                                    stats.dropped += 1;
                                    last_drop = Some(Instant::now());
                                    force_keyframe.store(true, Ordering::SeqCst);
                                    // The link is slower than the stream: ask for less, at most every
                                    // 2 s. A phone that reports what it gets lets this be measured instead.
                                    if !link_measured && last_rate_change.elapsed() >= Duration::from_secs(2)
                                    {
                                        let lower = (bitrate / 4 * 3).max(MIN_BITRATE);
                                        if lower < bitrate && enc.set_bitrate(lower) {
                                            bitrate = lower;
                                            last_rate_change = Instant::now();
                                        }
                                    }
                                }
                                MirrorSend::Closed => {
                                    send_failed = true;
                                    break;
                                }
                            }
                        }
                        if send_failed {
                            break;
                        }
                        // Video got ahead of the link, which says how much it carries. Leave
                        // room for sound and keyframes, and at most halve it: one bad moment
                        // on Wi-Fi shouldn't leave the picture blurry for a minute.
                        if let Some((rate, bytes)) = sender.link_measure() {
                            link_measured = true;
                            last_drop = Some(Instant::now());
                            let busy = bytes >= MIN_MEASURED_BYTES;
                            if busy && last_rate_change.elapsed() >= Duration::from_secs(1) {
                                let room = u32::try_from(rate / 10 * 7).unwrap_or(u32::MAX);
                                let lower = room.max(bitrate / 2).max(MIN_BITRATE);
                                if lower < bitrate && enc.set_bitrate(lower) {
                                    bitrate = lower;
                                    last_rate_change = Instant::now();
                                }
                            }
                        }
                        // Clean for a while: step the bitrate back up toward what the phone asked for.
                        let clean = last_drop.is_none_or(|t| t.elapsed() >= RAISE_AFTER_CLEAN);
                        if clean && bitrate < max_bitrate && last_rate_change.elapsed() >= RAISE_EVERY {
                            let higher = (bitrate / 4 * 5).min(max_bitrate);
                            if enc.set_bitrate(higher) {
                                bitrate = higher;
                                last_rate_change = Instant::now();
                            }
                        }
                    }
                    Err(e) => {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        #[cfg(linux_ffmpeg_video)]
                        if enc.next_encoder(&codec, dst_w, dst_h, fps, bitrate) {
                            tracing::warn!(error = %e, codec, "Video encoding failed; trying another encoder");
                            stats.rebuilds += 1;
                            continue;
                        }
                        if codec == "h265" {
                            tracing::warn!(error = %e, "HEVC screen encoding failed; using H.264");
                            codec = "h264".into();
                        } else {
                            tracing::warn!(error = %e, "PC mirror frame encode error; rebuilding the encoder");
                        }
                        encoder = None;
                    }
                }
            }

            if stats_since.elapsed() >= Duration::from_secs(10) {
                let secs = stats_since.elapsed().as_secs_f64();
                tracing::info!(
                    %peer,
                    fps = format!("{:.1}", stats.frames as f64 / secs),
                    kbps = (stats.bytes as f64 * 8.0 / secs / 1000.0) as u64,
                    keyframes = stats.keyframes,
                    dropped = stats.dropped,
                    rebuilds = stats.rebuilds,
                    bitrate,
                    "sharing this PC's screen"
                );
                stats = Stats::default();
                stats_since = Instant::now();
            }

            let elapsed = tick_start.elapsed();
            if let Some(rem) = frame_interval.checked_sub(elapsed) {
                std::thread::sleep(rem);
            }
        }

        // Only when no newer session took over and nothing records the screen:
        // the screencast is shared.
        let newest = ACTIVE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_none_or(|s| Arc::ptr_eq(&s.stop, &stop));
        if newest && !crate::pc_capture::is_recording() {
            stop_wayland_screencast();
        }
        tracing::info!(%peer, "stopped sharing this PC's screen");
        crate::input_share::leave_remote();
        sender.close();
        cleanup_session(&stop);
    }

    /// Mute this PC's speakers while a phone watches (the setting).
    static MUTE_WHILE_WATCHED: AtomicBool = AtomicBool::new(true);

    pub fn set_mute_while_watched(on: bool) {
        MUTE_WHILE_WATCHED.store(on, Ordering::Relaxed);
    }

    /// The PC's sound going to a phone that watches one of its players (as
    /// Phone speaker sends it), with the PC's speakers muted if the user wants
    /// (capturing the sound doesn't depend on them), for as long as this lives;
    /// then as before.
    /// The PC's sound goes to the phone while it watches, and comes back
    /// afterwards. A restart starts a new one before the old one ends, so
    /// the settings from before the first are kept here, and only the newest
    /// puts them back.
    struct SoundToPhone {
        generation: u64,
    }

    /// Whether the sound went to the phone, and where, before any of this;
    /// and whether this muted the speakers (they weren't already).
    static BEFORE: Mutex<Option<(bool, Option<DeviceId>, bool)>> = Mutex::new(None);
    static NEWEST: AtomicU64 = AtomicU64::new(0);

    impl SoundToPhone {
        fn start(peer: DeviceId) -> SoundToPhone {
            let generation = NEWEST.fetch_add(1, Ordering::AcqRel) + 1;
            let mut before = BEFORE.lock().unwrap_or_else(|e| e.into_inner());
            if before.is_none() {
                let muted = MUTE_WHILE_WATCHED.load(Ordering::Relaxed)
                    && sys::audio::speaker_state().is_some_and(|(_, muted)| !muted)
                    && sys::audio::set_speaker_mute(true).is_ok();
                *before =
                    Some((crate::pc_audio_stream::is_enabled(), crate::pc_audio_stream::target(), muted));
            }
            crate::pc_audio_stream::set_target(Some(peer));
            crate::pc_audio_stream::set_enabled(true);
            SoundToPhone { generation }
        }
    }

    impl Drop for SoundToPhone {
        fn drop(&mut self) {
            if NEWEST.load(Ordering::Acquire) != self.generation {
                return;
            }
            let Some((was_on, was_target, muted)) = BEFORE.lock().unwrap_or_else(|e| e.into_inner()).take()
            else {
                return;
            };
            crate::pc_audio_stream::set_target(was_target);
            crate::pc_audio_stream::set_enabled(was_on);
            // Unmuted only if this muted it.
            if muted {
                let _ = sys::audio::set_speaker_mute(false);
            }
        }
    }

    fn cleanup_session(stop: &Arc<AtomicBool>) {
        let removed = {
            let mut guard = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
            if guard.as_ref().is_some_and(|s| Arc::ptr_eq(&s.stop, stop)) {
                guard.take().is_some()
            } else {
                false
            }
        };
        if removed {
            core_host::host().hub.changed(crate::state::Changes::REMOTE);
            core_host::refresh_capabilities();
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn the_screen_choice_survives_a_restart_privately() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("token");
            assert_eq!(super::read_restore_token(&path), None);
            super::save_restore_token(&path, "first").unwrap();
            assert_eq!(super::read_restore_token(&path).as_deref(), Some("first"));
            super::save_restore_token(&path, "second").unwrap();
            assert_eq!(super::read_restore_token(&path).as_deref(), Some("second"));
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            std::fs::write(&path, "  ").unwrap();
            assert_eq!(super::read_restore_token(&path), None);
        }

        use super::*;

        #[test]
        fn recordings_take_the_screen_at_any_size() {
            // One row: red, then blue.
            let src = [0, 0, 255, 255, 255, 0, 0, 255];
            let mut dst = Vec::new();
            assert!(scale_bgra(&src, 2, 1, 4, 2, &mut dst));
            assert_eq!(dst.len(), 4 * 2 * 4);
            assert_eq!(dst[..8], [0, 0, 255, 255, 0, 0, 255, 255], "red, twice");
            assert_eq!(dst[24..], [255, 0, 0, 255, 255, 0, 0, 255], "blue, twice, a row down");
            assert!(scale_bgra(&src, 2, 1, 2, 1, &mut dst));
            assert_eq!(dst, src);
            assert!(!scale_bgra(&src, 3, 1, 2, 1, &mut dst), "too short for its size");
            assert!(!scale_bgra(&src, 2, 1, 0, 1, &mut dst));
        }

        #[test]
        fn wayland_capture_reads_the_stream_through_the_portal_connection() {
            let args = pipeline_args(57, 1280, 720);
            assert_eq!(args[1], "pipewiresrc");
            assert!(args.contains(&"fd=3".to_owned()), "the portal's PipeWire connection, as fd 3");
            assert!(args.contains(&"path=57".to_owned()));
            assert!(args.contains(&"video/x-raw,format=BGRx,width=1280,height=720".to_owned()));
            assert_eq!(args.last().map(String::as_str), Some("fd=1"));
        }

        #[test]
        fn computes_target_dimensions_and_nv12() {
            let (w, h) = target_dimensions(1920, 1080, 1280);
            assert_eq!(w % 16, 0);
            assert_eq!(h % 16, 0);
            let bgra = vec![128u8; (w as usize) * (h as usize) * 4];
            let mut nv12 = Vec::new();
            bgra_to_nv12(&bgra, w as usize, h as usize, &mut nv12);
            assert_eq!(nv12.len(), (w as usize) * (h as usize) * 3 / 2);
        }

        #[test]
        fn recognizes_hevc_random_access_frames_and_parameter_sets() {
            let headers = [
                0, 0, 0, 1, 0x40, 1, // VPS
                0, 0, 1, 0x42, 1, // SPS
                0, 0, 0, 1, 0x44, 1, // PPS
            ];
            let idr = [0, 0, 1, 0x26, 1]; // IDR_W_RADL, type 19
            assert!(has_h265_parameters(&headers));
            assert!(!has_h265_irap(&headers));
            assert!(has_h265_irap(&idr));
            assert!(!has_h265_parameters(&idr));
        }

        #[test]
        fn hevc_encoder_round_trips_when_available() {
            let (w, h) = target_dimensions(1280, 720, 1280);
            let mut enc = match ScreenEncoder::new_for_codec("h265", w, h, 30, 2_000_000) {
                Ok(encoder) => encoder,
                Err(error) => {
                    eprintln!("HEVC encoder unavailable: {error}");
                    return;
                }
            };
            let mut dec = match crate::sys::h264::Decoder::new_for_codec("h265") {
                Ok(decoder) => decoder,
                Err(error) => {
                    eprintln!("HEVC decoder unavailable: {error}");
                    return;
                }
            };
            let nv12 = vec![128u8; (w as usize) * (h as usize) * 3 / 2];
            let mut produced_packets = 0usize;
            let mut decoded_picture = false;
            let stop = AtomicBool::new(false);
            for i in 0..8 {
                for (keyframe, data) in enc
                    .encode(&nv12, i * 333_333, 333_333, &stop)
                    .unwrap_or_else(|error| panic!("encode HEVC frame {i}: {error}"))
                {
                    produced_packets += 1;
                    if keyframe {
                        assert!(has_h265_irap(&data), "keyframes must contain an HEVC IRAP NAL");
                        assert!(has_h265_parameters(&data), "keyframes must carry VPS, SPS and PPS");
                    }
                    decoded_picture |=
                        !dec.decode(&data, i as u64 * 33_333).expect("decode packet").is_empty();
                }
            }
            assert!(produced_packets > 0, "encoder must emit H.265 access units");
            assert!(decoded_picture, "decoder must reconstruct an H.265 picture");
        }

        #[test]
        fn encodes_nv12_frames_and_decodes_back() {
            let (w, h) = target_dimensions(1920, 1080, 1280);
            assert_eq!(w % 16, 0);
            assert_eq!(h % 16, 0);

            let mut enc = new_pc_encoder("h264", w, h, 30, 4_000_000).expect("ScreenEncoder");
            let mut dec = crate::sys::h264::Decoder::new_for_codec("h264").expect("H264 Decoder");

            let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
            for (i, px) in bgra.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                px[0] = (i % 255) as u8;
                px[1] = ((i / 7) % 255) as u8;
                px[2] = ((i / 13) % 255) as u8;
                px[3] = 255;
            }
            let mut nv12 = Vec::new();
            bgra_to_nv12(&bgra, w as usize, h as usize, &mut nv12);

            let mut decoded_any = false;
            let mut produced_packets = 0usize;
            let stop = AtomicBool::new(false);
            for i in 0..8 {
                let packets = enc.encode(&nv12, i * 333_333, 333_333, &stop).expect("encode frame");
                for (_keyframe, data) in packets {
                    produced_packets += 1;
                    let pics = dec.decode(&data, (i as u64) * 33_333).expect("decode packet");
                    if let Some(pic) = pics.first() {
                        assert_eq!(pic.width, w);
                        assert_eq!(pic.height, h);
                        decoded_any = true;
                    }
                }
            }
            assert!(produced_packets > 0, "encoder must emit H.264 access units");
            assert!(decoded_any, "decoder must reconstruct at least one frame from ScreenEncoder");
        }

        #[test]
        fn keyframes_come_on_request_without_rebuilding_the_encoder() {
            let (w, h) = target_dimensions(1280, 720, 1280);
            let mut enc =
                ScreenEncoder::OpenH264(OpenH264Encoder::new(w, h, 30, 2_000_000).expect("OpenH264"));
            let mut nv12 = Vec::new();
            bgra_to_nv12(&vec![128u8; (w as usize) * (h as usize) * 4], w as usize, h as usize, &mut nv12);
            let stop = AtomicBool::new(false);

            let keyframes_after = |enc: &mut ScreenEncoder, start: i64, count: i64| {
                (start..start + count)
                    .flat_map(|i| enc.encode(&nv12, i * 333_333, 333_333, &stop).expect("encode frame"))
                    .filter(|(keyframe, data)| *keyframe && has_h264_idr(data))
                    .count()
            };
            // The first frame is a keyframe; the next few (well inside the 2 s GOP) are not.
            assert_eq!(keyframes_after(&mut enc, 0, 1), 1, "the stream starts with a keyframe");
            assert_eq!(keyframes_after(&mut enc, 1, 5), 0, "no keyframes mid-GOP unprompted");
            assert!(enc.request_keyframe(), "the H.264 encoder takes keyframe requests");
            assert_eq!(keyframes_after(&mut enc, 6, 2), 1, "a requested keyframe arrives promptly");
            assert!(enc.set_bitrate(1_000_000), "the bitrate changes in place");
        }
    }
}

pub use imp::*;
