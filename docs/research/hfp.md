# Bluetooth HFP & PC Call Audio Architecture (`docs/research/hfp.md`)

## 1. Why Android Blocks Software Wi-Fi Call Audio Capture

On stock Android (non-rooted devices without OEM system-signature privileges), cellular and PSTN voice call audio (`AudioAttributes.USAGE_VOICE_COMMUNICATION` / `AudioSource.VOICE_CALL` / `AudioSource.VOICE_DOWNLINK` / `AudioSource.VOICE_UPLINK`) is hardware-routed by the baseband/modem DSP and the Android audio HAL:

1. **`CAPTURE_AUDIO_OUTPUT` & `CONTROL_INCALL_EXPERIENCE` are signature/privileged permissions**: Third-party apps, even when granted `READ_PHONE_STATE`, `ANSWER_PHONE_CALLS`, `CALL_PHONE`, or `InCallService` role, cannot read the downlink voice stream or inject uplink microphone PCM into an active cellular call over Wi-Fi/LAN.
2. **`AudioPlaybackCapture` (`MediaProjection`) excludes voice calls**: Android 10+'s `AudioPlaybackCaptureConfiguration` explicitly blocks capturing `USAGE_VOICE_COMMUNICATION` and `USAGE_VOICE_COMMUNICATION_SIGNALLING`.
3. **The shell user (`Elevated`) may record the call sources, but whether they carry the call depends on the phone**: `shell` holds `CAPTURE_AUDIO_OUTPUT` (the Elevated helper already records `REMOTE_SUBMIX` with it), which is the permission `AudioPolicyService` asks for `VOICE_CALL`, `VOICE_UPLINK` and `VOICE_DOWNLINK`. Checked on 2026-11-02 on an Android 17 emulator: a recorder on each of the three, created from `app_process` as `shell`, initializes and records, during a call and outside one. The emulator's audio HAL feeds its microphone input there either way, so it can't show whether a real phone's HAL delivers the call; that is decided per device, in the audio HAL and the modem's DSP. On a real phone (Samsung Galaxy A35, SM-A356E, Exynos 1380, Android 17, Jio VoLTE call to an automated line, 2026-11-02), `VOICE_DOWNLINK` and `VOICE_CALL` carried the other side's audio as `shell` (RMS 280 to 3500 against 8 to 18 with no call, not silenced), and `VOICE_UPLINK` returned only zeros.
4. **Feeding sound into a call is unconfirmed**: apps have no public API for it. Phones expose a telephony output (`AudioDeviceInfo.TYPE_TELEPHONY`), which system features use to play into a call. On the Galaxy A35 above, a `shell` `AudioTrack` with `USAGE_VOICE_COMMUNICATION` was routed to it during the call without an error, but whether the other side hears it is untested (the phone's call recordings carry only the other side, so they can't show it).

Consequently, hearing a cellular call on the PC works with `Elevated` on phones whose audio HAL delivers the call to those sources (confirmed on one Samsung), and talking into it from the PC's microphone without Bluetooth is unconfirmed.

---

## 2. Call Audio on the PC over Bluetooth HFP

The way around section 1 is Bluetooth: the phone sends call audio to a hands-free device by itself, and the PC can be that device. G7 Link's part is small. `desktop/app/src/linux/bt_hfp.rs`:

1. **Finds the phone among the PC's paired Bluetooth devices**, through BlueZ over D-Bus (`org.bluez`, `ObjectManager`), or `bluetoothctl` when that fails. It matches the device by the phone's name.
2. **Connects it** (`org.bluez.Device1.Connect`, or `bluetoothctl connect`).
3. **Switches the phone's audio card to a call profile** with `pactl set-card-profile bluez_card.<MAC>`, trying the profile names PipeWire and PulseAudio use for HFP and HSP in turn, with `a2dp-sink` last.
4. **Switches back to `a2dp-sink`** when call audio is disconnected.

Whether a call is heard on the PC depends on the desktop's Bluetooth audio stack offering a call profile for the phone. G7 Link doesn't install or configure one.

Without Bluetooth, G7 Link supports incoming call alerts, caller ID, answer/decline/hangup, dialer, call log, and (with Elevated mode) mute/speakerphone/hold/DTMF keypad controls on the PC while call voice audio remains on the phone (or a Bluetooth headset paired directly to the phone).
