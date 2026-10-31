# Webcam filter toggle evidence

The screenshots use the synthetic `profile_preview --demo --page=voice` fixture at
1120 × 850, scale 1, dark theme, Xvfb and Mesa software rendering. No camera,
microphone or Discord account was used. The before image is from fork main
`39060d31`; the after image is from this change. A 720 × 900 light-theme
preview was also inspected for wrapping and alignment.

The saved preference remains Off by default. Switching it on requests
`atadenoise,hqdn3d=4,unsharp=7:7:0.5` when the camera or local preview next
starts. Physical camera quality and timing require owner-controlled validation.
See `docs/performance.md` for the separate synthetic filter timings.
