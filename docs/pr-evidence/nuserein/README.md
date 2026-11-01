# NuSerein branding evidence

The original `before.png` uses fork main at `9fd017f7`; `after.png` shows the
name change at `a773d1ae`, before the new icon was added. Both show
`--demo --demo-login` at 1280 × 800 on Linux/Xvfb, with the Vulkan llvmpipe
renderer and the same synthetic accounts.

`signal-n-after.png` shows the same fixture, viewport and renderer at `5ea1a8de`
after installing the selected Signal N artwork. It can be compared with `after.png`
to see the icon change. The full-color icon uses one 64 × 64 RGBA UI texture plus
mipmaps; the monochrome atlas and platform icons also contain the new mark.

The installation and data identifiers remain unchanged. The in-app updater and
AppImage metadata target the fork. A Flatpak reference for the existing upstream
repository is identified as upstream; a separately hosted fork repository must
be supplied explicitly.

These screenshots do not validate Windows/macOS installation or live Discord
behavior. Release package sizes and performance were not measured here; no
performance improvement is claimed.
