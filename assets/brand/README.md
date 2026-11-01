# NuSerein brand assets

Signal N is the selected NuSerein mark: an N and audio waveform inside a speech bubble.
`signal-n-original.png` is the approved generated artwork. The full-color platform icons are
cropped and resized from it; `serein-1024.png` is the normalized 1024×1024 source used by
those packages. Existing filenames and bundle identifiers remain unchanged for upgrades.

`serein.svg` is a simplified vector adaptation for Linux's scalable icon and very small
16–24 pixel icons. `serein-flat.svg` is its flat-color variant. `serein-mark.svg` is the
matching one-color N and waveform used by the interface atlas, Windows tray and macOS menu-bar
template. The template is `serein-tray.png`; macOS tints it for the current appearance.

To regenerate the packaged PNG, ICO, ICNS, macOS Icon Composer image, tray image and brand
cell in the existing atlas, install ImageMagick 7 (`magick`) and Inkscape, then run:

```sh
python3 tools/generate-brand-assets.py
```

The script runs offline, preserves the other atlas cells, and uses a simplified rendition at
small sizes for legibility. The platform assets live in `packaging/linux/hicolor/`,
`packaging/windows/`, and `packaging/macos/`. Native Windows/macOS rendering still needs
platform verification; a Linux asset check does not exercise their icon loaders.
