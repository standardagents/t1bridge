# Camera application acceptance

Next step for [#20](https://github.com/standardagents/t1bridge/issues/20).
An advertised camera node and successful H.264 decoding do not establish
ordinary desktop/browser compatibility.

## Read-only baseline, September 20

On MacBookPro13,3, kernel `7.2.3-arch1-3`, core/DKMS
`0.1.9.r8.g995f306-1`, video nodes discovered through their T1 USB ancestor
advertised H.264 at 1280×720 and 640×480, with discrete intervals from 1 to 30
frames/second. The other T1 video node reported no capture formats in this
query. Device paths were discarded. No stream was started or image captured.

Installed integration was PipeWire `1:1.6.8-1`, WirePlumber `0.5.17-1`,
Hyprland portal `1.4.1-2`, Chromium `152.0.7977.82-1`, and
`gst-plugin-pipewire 1:1.6.8-1`. `gst-libav` and Firefox were absent. Package
presence/absence alone does not show which decoder a browser uses or prove
that installing an additional decoder will fix negotiation.

This older development build is an investigation baseline. Run acceptance on
the exact official cohort before changing support claims or dependencies.

## Attended matrix

Start only after the owner is ready to observe the camera indicator and
preview. Use local previews, retain no frames, and stop capture after each row.
Use an existing enrolled fingerprint for coexistence; do not enroll or change
PAM for this matrix.

| Consumer | Check | Current result |
| --- | --- | --- |
| Ordinary V4L2 consumer | Discover T1 dynamically; negotiate each advertised size; decode a short preview without a persistent conversion pipeline. | Metadata only; capture pending. |
| PipeWire consumer | Discover through the normal device graph; record negotiated encoded/raw format and usable preview, then release the device. | Pending. |
| Omarchy's shipped browser | Use `getUserMedia` on a trusted local origin; record camera enumeration, consent, selected backend, negotiated dimensions and actual preview. | Pending. |
| Portal-aware sandboxed application | Verify the ordinary camera-permission path and preview independently of browser direct-V4L2 access. | Pending. |
| Reopen/coexistence | Five start/stop cycles, application switching, a five-minute preview with ordinary Touch Bar use and one attended fingerprint verify. | Pending. |

Record model, kernel, exact package/application versions, negotiated format,
preview/indicator behavior, start/stop count and a sanitized error category.
Reject a result requiring per-app pipelines, manual module replacement, a
virtual camera, or a broad camera-permission bypass as out-of-box acceptance.
If H.264 negotiation fails, locate the failing consumer/backend before adding
dependencies; the absence of raw or MJPEG formats alone does not identify a
single fix.

Cold-boot and upgrade checks follow a passing application row. Suspend and
runtime-power recovery depend on #18 and #19; mark those pending separately.
