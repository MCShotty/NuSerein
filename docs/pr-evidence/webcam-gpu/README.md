# GPU webcam switch evidence

The synthetic `profile_preview --demo --page=voice` capture uses a 1120 × 850
window, dark theme and scale 1 under Xvfb with software rendering. The fixture
has no hardware GPU capability, so the camera switch is disabled and its reason
is visible. The [previous capture](../webcam-filter-toggle/after.png) is from
`57958465`, before hardware availability was checked. A 720 × 900 light-theme
view was also inspected. No camera, microphone or Discord account was used.

This is UI evidence, not a hardware-GPU or webcam-quality measurement. See
`docs/performance.md` for the separate filter and LavaSRv2 test limitations.
