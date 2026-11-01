#!/usr/bin/env python3
"""Generate NuSerein's platform icons from the approved Signal N artwork.

Requires ImageMagick 7 (magick) and Inkscape. No network access is used.
"""

from pathlib import Path
import shutil
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BRAND = ROOT / "assets/brand"
SOURCE = BRAND / "signal-n-original.png"
VECTOR = BRAND / "serein.svg"
MARK = BRAND / "serein-mark.svg"
LINUX = ROOT / "packaging/linux/hicolor"
WINDOWS = ROOT / "packaging/windows"
MACOS = ROOT / "packaging/macos"
ATLAS = ROOT / "assets/icons/atlas.png"
ICON_SIZES = (16, 20, 22, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 128, 192, 256, 512, 1024)
LINUX_SIZES = (16, 22, 24, 32, 48, 64, 96, 128, 256, 512, 1024)
ICO_SIZES = (16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 128, 192, 256)


def run(*command):
    subprocess.run(command, check=True, stdout=subprocess.DEVNULL)


def render_svg(source, size, output):
    run("inkscape", str(source), "--export-area-page", f"--export-width={size}",
        f"--export-height={size}", f"--export-filename={output}")


def main():
    for tool in ("magick", "inkscape"):
        if not shutil.which(tool):
            raise SystemExit(f"Install {tool} to regenerate the brand assets")
    with tempfile.TemporaryDirectory(prefix="nuserein-icons-") as directory:
        temporary = Path(directory)
        cropped, mask = temporary / "cropped.png", temporary / "mask.png"
        # The approved image has excess transparent border and faint alpha noise.
        # Crop symmetrically around the tile; leave a small margin and soft edges.
        run("magick", str(SOURCE), "-crop", "1000x1000+127+136", "+repage", f"PNG32:{cropped}")
        run("magick", str(cropped), "-alpha", "extract", "-level", "1%,100%", str(mask))
        canonical = BRAND / "serein-1024.png"
        run("magick", str(cropped), str(mask), "-alpha", "off", "-compose", "CopyOpacity",
            "-composite", "-filter", "Lanczos", "-resize", "1024x1024!", "-strip", f"PNG32:{canonical}")
        rendered = {}
        for size in ICON_SIZES:
            target = temporary / f"{size}.png"
            if size <= 24:
                # A clearer, simplified version of the selected mark for very small icons.
                render_svg(VECTOR, size, target)
            elif size == 1024:
                shutil.copyfile(canonical, target)
            else:
                run("magick", str(canonical), "-filter", "Lanczos", "-resize", f"{size}x{size}!",
                    "-strip", f"PNG32:{target}")
            rendered[size] = target
        for size in LINUX_SIZES:
            shutil.copyfile(rendered[size], LINUX / f"{size}x{size}/apps/serein.png")
        shutil.copyfile(rendered[256], WINDOWS / "serein.png")
        run("magick", *(str(rendered[size]) for size in ICO_SIZES), str(WINDOWS / "Serein.ico"))
        # ICNS stores PNG chunks with big-endian type/length headers. Include both
        # standard and Retina names so the system can pick the proper pixel size.
        chunks = []
        for tag, size in ((b"icp4", 16), (b"icp5", 32), (b"icp6", 64), (b"ic07", 128),
                          (b"ic08", 256), (b"ic09", 512), (b"ic10", 1024), (b"ic11", 32),
                          (b"ic12", 64), (b"ic13", 256), (b"ic14", 512)):
            data = rendered[size].read_bytes()
            chunks.append(tag + struct.pack(">I", len(data) + 8) + data)
        payload = b"".join(chunks)
        (MACOS / "Serein.icns").write_bytes(b"icns" + struct.pack(">I", len(payload) + 8) + payload)
        shutil.copyfile(canonical, MACOS / "Serein.icon/Assets/signal-n.png")
        tray = temporary / "tray.png"
        render_svg(MARK, 72, tray)
        run("magick", str(tray), "-channel", "RGB", "-fill", "black", "-colorize", "100",
            "+channel", "-strip", f"PNG32:{BRAND / 'serein-tray.png'}")
        # Preserve every other atlas cell and replace only the tintable brand glyph.
        glyph, cell, atlas = temporary / "glyph.png", temporary / "cell.png", temporary / "atlas.png"
        render_svg(MARK, 56, glyph)
        run("magick", "-size", "64x64", "xc:none", str(glyph), "-geometry", "+4+4",
            "-compose", "Over", "-composite", f"PNG32:{cell}")
        run("magick", str(ATLAS), str(cell), "-geometry", "+128+448",
            "-define", "compose:outside-overlay=false", "-compose", "Copy", "-composite",
            "-strip", f"PNG32:{atlas}")
        shutil.copyfile(atlas, ATLAS)
    print("Generated NuSerein icons for Linux, Windows, macOS, tray and the UI atlas.")


if __name__ == "__main__":
    main()
