"""Disposable CI evidence helper; never starts the desktop or an owner session."""

import hashlib
import json
import os
from pathlib import Path
import platform
import re
import statistics
import subprocess
import sys
import tomllib
import zipfile


def run(args, *, cwd=None, env=None, capture=False, timeout=5400):
    return subprocess.run(
        [str(arg) for arg in args], cwd=cwd, env=env, check=True,
        capture_output=capture, text=True, encoding="utf-8", timeout=timeout,
    )


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def parse_replay(output):
    match = re.search(
        r"Synthetic reducer replay: 100000 events in ([0-9.]+)(ns|µs|us|ms|s); "
        r"retained timeline (\d+)\.\.(\d+) estimated bytes, (\d+) records", output,
    )
    if not match:
        raise ValueError("Missing the bounded 100,000-event replay result")
    elapsed, units, lower, upper, records = match.groups()
    scale = {"ns": 1e-6, "µs": 1e-3, "us": 1e-3, "ms": 1, "s": 1000}
    result = {
        "elapsed_ms": float(elapsed) * scale[units],
        "retained_min": int(lower), "retained_max": int(upper),
        "records": int(records),
    }
    if result["records"] != 500 or not 0 <= result["retained_min"] <= result["retained_max"] <= 4 * 1024 * 1024:
        raise ValueError("Replay retention invariants failed")
    return result


def prepare():
    root = Path(os.environ["RUNNER_TEMP"]) / "serein-native-comparison"
    native = root / "ffmpeg"
    variables = {
        "FFMPEG_DIR": native / "prefix", "FFMPEG_WORK": native / "build",
        "FFMPEG_CACHE": native / "sources",
    }
    with Path(os.environ["GITHUB_ENV"]).open("a", encoding="utf-8") as stream:
        for key, value in variables.items():
            stream.write(f"{key}={value}\n")
    if sys.platform == "win32":
        with Path(os.environ["GITHUB_PATH"]).open("a", encoding="utf-8") as stream:
            stream.write(str(variables["FFMPEG_DIR"] / "bin") + "\n")
    (Path(os.environ["RUNNER_TEMP"]) / "native-comparison-evidence").mkdir(exist_ok=True)


def environment():
    result = {
        "platform": platform.uname()._asdict(), "logical_cpus": os.cpu_count(),
        "runner_os": os.environ["RUNNER_OS"], "runner_arch": os.environ["RUNNER_ARCH"],
        "image_os": os.environ.get("ImageOS"), "image_version": os.environ.get("ImageVersion"),
        "python": platform.python_version(),
        "rustc": run(["rustc", "+1.98.1", "--version"], capture=True, timeout=30).stdout.strip(),
    }
    if sys.platform == "win32":
        command = "[ordered]@{cpu=(Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name); memory_bytes=(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory; os=(Get-CimInstance Win32_OperatingSystem).Caption; build=(Get-CimInstance Win32_OperatingSystem).BuildNumber} | ConvertTo-Json -Compress"
        result["hardware"] = json.loads(run(["pwsh", "-NoProfile", "-Command", command], capture=True, timeout=30).stdout)
    elif sys.platform == "darwin":
        result["hardware"] = {
            "cpu": run(["sysctl", "-n", "machdep.cpu.brand_string"], capture=True, timeout=30).stdout.strip(),
            "memory_bytes": int(run(["sysctl", "-n", "hw.memsize"], capture=True, timeout=30).stdout),
            "os": run(["sw_vers"], capture=True, timeout=30).stdout.strip(),
        }
    else:
        raise RuntimeError("This comparison requires a native Windows or macOS runner")
    return result


def package(source, target, out):
    env = dict(os.environ, CARGO_TARGET_DIR=str(target), SEREIN_SIGNING_IDENTITY="")
    assert not target.exists(), "Each source needs a fresh, separate Cargo target"
    assert not (source / "dist").exists() and not (source / "dist-installer").exists()
    assert not run(["git", "status", "--porcelain"], cwd=source, capture=True, timeout=30).stdout.strip()
    run(["cargo", "+1.98.1", "build", "--release", "--locked", "-p", "replay-bench"], cwd=source, env=env)
    run(["cargo", "+1.98.1", "build", "--locked", "-p", "xtask"], cwd=source, env=env)
    run(["cargo", "+1.98.1", "xtask", "package"], cwd=source, env=env)
    root = source / "dist"
    compiled = target / "release" / ("serein.exe" if sys.platform == "win32" else "serein")
    exe = root / "serein.exe" if sys.platform == "win32" else root / "Serein.app/Contents/MacOS/serein"
    if sys.platform == "win32":
        assert digest(exe) == digest(compiled), "Staged executable differs from its compiled source"
    else:
        # Normal macOS packaging ad-hoc signs the staged copy, changing its hash.
        run(["codesign", "--verify", "--strict", root / "Serein.app"], timeout=60)
    files = [path for path in root.rglob("*") if path.is_file() and not path.is_symlink()]
    payload = sum(path.stat().st_size for path in files)
    assert 0 < exe.stat().st_size <= payload < 1024 * 1024 * 1024
    out.mkdir(parents=True)
    archive = out / "serein.zip"
    if sys.platform == "win32":
        zip_env = dict(env, SEREIN_COMPARE_ROOT=str(root), SEREIN_COMPARE_ZIP=str(archive))
        command = "Compress-Archive -LiteralPath (Get-ChildItem -LiteralPath $env:SEREIN_COMPARE_ROOT | Select-Object -ExpandProperty FullName) -DestinationPath $env:SEREIN_COMPARE_ZIP -CompressionLevel Optimal"
        run(["pwsh", "-NoProfile", "-Command", command], env=zip_env, timeout=300)
    else:
        run(["ditto", "-c", "-k", "--sequesterRsrc", root, archive], timeout=300)
    with zipfile.ZipFile(archive) as zipped:
        assert zipped.testzip() is None, "Compressed distribution failed its CRC check"
    result = {
        "executable_bytes": exe.stat().st_size, "installed_bytes": payload,
        "installed_files": len(files), "zip_bytes": archive.stat().st_size,
        "compiled_sha256": digest(compiled), "packaged_sha256": digest(exe),
        "zip_sha256": digest(archive),
    }
    if sys.platform == "win32":
        installers = list((source / "dist-installer").glob("*-setup.exe"))
        assert len(installers) == 1, "Normal packaging must create exactly one installer"
        result["installer_bytes"] = installers[0].stat().st_size
        result["installer_sha256"] = digest(installers[0])
    # The normal Windows packager creates an untracked dist-installer directory.
    # Check source integrity without mistaking this task-owned output for an edit.
    assert not run(["git", "status", "--porcelain", "--untracked-files=no"], cwd=source, capture=True, timeout=30).stdout.strip()
    return result


def main():
    workspace = Path(os.environ["GITHUB_WORKSPACE"])
    scratch = Path(os.environ["RUNNER_TEMP"]) / "serein-native-comparison"
    evidence = Path(os.environ["RUNNER_TEMP"]) / "native-comparison-evidence"
    sources = {"nightly": workspace / "nightly", "pr": workspace / "candidate"}
    revisions = {"nightly": os.environ["BASELINE_SHA"], "pr": os.environ["CANDIDATE_SHA"]}
    targets = {label: scratch / "targets" / label for label in sources}
    profiles = {}
    for label, source in sources.items():
        assert run(["git", "rev-parse", "HEAD"], cwd=source, capture=True, timeout=30).stdout.strip() == revisions[label]
        profiles[label] = tomllib.loads((source / "Cargo.toml").read_text())["profile"]["release"]
    assert profiles["nightly"] == profiles["pr"], "Release profiles differ"
    assert (sources["nightly"] / "tools/replay-bench/src/main.rs").read_bytes() == (sources["pr"] / "tools/replay-bench/src/main.rs").read_bytes()
    result = {
        "baseline_tag": "v1.0.0-nightly.20261010.57", "revisions": revisions,
        "environment": environment(), "release_profile": profiles["pr"],
        "features": "standard voice-inclusive release; no demo/developer features",
        "method": "one normal package per revision from empty outputs; .NET Optimal ZIP on Windows, ditto --sequesterRsrc ZIP on macOS; one replay warmup per binary, five alternating pairs after all builds finish",
        "scope": "package sizes and 100000-event core reducer; excludes media FPS, hardware encoding/decoding, UI latency, process/GPU memory and Discord interoperability",
        "packages": {},
    }
    (evidence / "partial.json").write_text(json.dumps(result, indent=2) + "\n")
    for label, source in sources.items():
        print(f"Building the exact {label} source {revisions[label]}", flush=True)
        result["packages"][label] = package(source, targets[label], scratch / "archives" / label)
        (evidence / "partial.json").write_text(json.dumps(result, indent=2) + "\n")
    suffix = ".exe" if sys.platform == "win32" else ""
    binaries = {label: target / "release" / ("replay-bench" + suffix) for label, target in targets.items()}
    for binary in binaries.values():
        parse_replay(run([binary], capture=True, timeout=30).stdout)
    samples = []
    for pair in range(5):
        order = ("nightly", "pr") if pair % 2 == 0 else ("pr", "nightly")
        for label in order:
            output = run([binaries[label]], capture=True, timeout=30).stdout
            samples.append(dict(parse_replay(output), pair=pair + 1, build=label))
    result["replay"] = {"warmups_per_binary": 1, "pairs": 5, "samples": samples}
    for label, binary in binaries.items():
        rows = [sample for sample in samples if sample["build"] == label]
        result["replay"][label] = {
            "median_ms": statistics.median(row["elapsed_ms"] for row in rows),
            "range_ms": [min(row["elapsed_ms"] for row in rows), max(row["elapsed_ms"] for row in rows)],
            "retained_range": [min(row["retained_min"] for row in rows), max(row["retained_max"] for row in rows)],
            "binary_sha256": digest(binary),
        }
    (evidence / "comparison.json").write_text(json.dumps(result, indent=2) + "\n")
    print("NATIVE_COMPARISON_RESULT=" + json.dumps(result, separators=(",", ":")), flush=True)


if __name__ == "__main__":
    if sys.argv[1:] == ["--prepare"]:
        prepare()
    elif not sys.argv[1:]:
        main()
    else:
        raise SystemExit("Usage: measure-native-nightly.py [--prepare]")
