# Calls

The **Calls** page on your PC lets you see incoming phone calls with caller ID, answer or decline them from your desk, reject a call with a quick SMS reply, dial numbers from a keypad, browse your recent call history and contacts, and control active calls.

<p align="center">
  <img src="images/calls-page.png" alt="Calls page on the PC showing recent call history, contacts, and dialer keypad" width="760">
</p>

## What You Need

| Requirement | Details |
| :--- | :--- |
| **Android Permissions** | **Calls on your PC** (`READ_PHONE_STATE`, `READ_CALL_LOG`, `ANSWER_PHONE_CALLS`, `CALL_PHONE`) and **Contacts on your PC** (`READ_CONTACTS`) in **Settings › Permissions › Calls, contacts and messages**. |
| **Android Power Level** | `Standard` for incoming call alerts, caller ID, answer, decline, hang up, call log, dialer, and volume control. `Elevated` (on Android 12+) enables in-call **Mute**, **Speakerphone**, **Hold**, and **Keypad (DTMF)** controls from the PC. |
| **Per-Device Toggles** | **Calls** and **Contacts** (both enabled by default in **Settings › Devices** on the PC). |
| **Network** | Local network, USB cable, or **Connect over mobile data**. |

## Step-by-Step Use

### Handling Incoming Calls on Your PC

1. When your phone rings, G7 Link displays an incoming call banner on your PC (and expands `G7 Island` if enabled) showing the caller's name, phone number, and contact photo.
2. If **Pause media during calls** is enabled in **Settings › Camera & Audio**, any music or video playing on your PC pauses automatically while the phone is ringing or in a call, and resumes when the call ends.
3. From the incoming call banner on your PC, you can:
   - Click **Answer** to pick up the call on your phone.
   - Click **Decline** to send the caller to voicemail.
   - Click **Silence** to mute the ringer without declining the call.
   - Click **Reply with text** (when SMS access is enabled) to decline the call and immediately send a preset or custom SMS reply to the caller.

### Controlling an Active Call

While a call is active on your phone, the call bar on your PC shows the live call timer and provides controls based on your phone's power level:

- **At `Standard` or `Assist`**: You can see the active call duration, adjust the call volume, and click **Hang up** to end the call.
- **At `Elevated` (Android 12 and newer)**: Additional in-call controls appear on the PC card, allowing you to toggle **Mute**, switch the phone to **Speaker** (speakerphone), place the call on **Hold**, or open the in-call **Keypad** to send DTMF tones for automated phone menus.

### Placing a Call from Your PC

1. Open **Calls** in the left navigation bar on your PC.
2. You can start a call in three ways:
   - **From the Keypad**: Type or paste a phone number into the dialer keypad (or press number keys on your keyboard) and click **Call**.
   - **With two SIMs**: Pick the SIM under **Call with** next to the keypad. The PC remembers it for that phone and uses it for every call it starts, so the phone doesn't stop to ask which SIM.
   - **From Contacts**: Search your synced phone contacts (favorites appear at the top) and click the call button next to any number.
   - **From Recent Calls**: Click any entry in your recent call history to call back or open a text conversation with that person.
3. Your phone places the cellular call immediately.

### Browsing Call History

The **Calls** page groups your phone's recent incoming, outgoing, and missed calls by day. Missed calls are highlighted in red. The number beside **Calls** in the sidebar counts the missed calls that came in since you last opened the Calls page for that phone on this PC, and opening the page clears it. A voicemail notification from your phone appears in the notification feed like any other notification.

## Call Audio

When you answer or place a call from your PC, G7 Link controls the call on your phone. The voice audio stays on your phone's earpiece, speakerphone, or Bluetooth headset unless you connect it to the PC over Bluetooth.

G7 Link can't carry call audio over its own connection. On non-rooted Android devices, cellular call audio (`VOICE_CALL`, `VOICE_DOWNLINK`, `VOICE_UPLINK`) is routed by the modem DSP and blocked from software capture (`AudioPlaybackCapture` excludes voice communication, even at the `Elevated` shell level). [`docs/research/hfp.md`](../research/hfp.md) has the details.

Bluetooth is the way around this. If your phone is paired with your PC in the system's Bluetooth settings, the call card and the Calls page show **Connect PC audio**. It connects to the phone through BlueZ and switches the phone's audio profile in PipeWire or PulseAudio to a hands-free one, so call audio can go through your PC's speakers and microphone. Whether that works depends on your Bluetooth audio stack. If the phone isn't paired over Bluetooth, the button opens your Bluetooth settings instead.

Otherwise, click **Speaker** in G7 Link's call bar (with `Elevated`) or keep your wireless headphones paired to your phone via multipoint Bluetooth.

## What the Options Do

- **Calls toggle** (`Settings › Devices` on PC, **On** by default): Enables caller ID, call log sync, incoming call alerts, and call control for that phone.
- **Contacts toggle** (`Settings › Devices` on PC, **On** by default): Syncs your phone's contacts so caller ID and the dialer show names and photos.
- **Pause media during calls** (`Settings › Camera & Audio` on PC, **On** by default): Automatically pauses media playing on your PC (MPRIS players) when a call rings or connects on your phone, and resumes playback when the call finishes unless you manually changed playback in the meantime.
- **Island triggers › Calls** (`Settings › General` on PC, **On** by default when `G7 Island` is enabled): Shows incoming and active phone calls inside the top-of-screen `G7 Island` pill.

## Data and Privacy

- **Where data travels**: Call state changes, caller ID metadata, recent call log entries, and contact records travel directly between your phone and PC over your end-to-end encrypted QUIC connection.
- **Where data stays**: Recent call logs and contacts are read live from your phone and held in memory on the PC during your session. G7 Link never records call audio.

## Limits

- **Two-Way Call Audio on PC**: As explained above, Android doesn't let apps capture cellular voice audio, so call audio reaches the PC only over a Bluetooth hands-free connection.
- **In-Call Mute, Hold, Speaker, and DTMF Keypad**: Standard Android permissions allow third-party apps to answer and end calls (`TelecomManager`), but muting the microphone, toggling speakerphone, holding a call, and sending DTMF tones during a cellular call require `Elevated` on Android 12 or newer unless the app replaces your default phone dialer.
- **Dual-SIM Selection**: When you place a call from the PC on a dual-SIM phone, Android uses the default calling SIM configured in your phone's SIM settings (or prompts on the phone if your SIM preference is set to "Ask every time").
