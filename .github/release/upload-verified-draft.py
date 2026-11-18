"""Publish verified native artifacts to one release draft without making it public."""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import struct
import subprocess
import tarfile
import tomllib
import zipfile

REPO = "MCShotty/NuSerein"
ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "release-assets"
CRT = {"msvcp140.dll", "msvcp140_1.dll", "vcruntime140.dll"}
MIB = 1024 ** 2


def api(path):
    return json.loads(subprocess.check_output(["gh", "api", f"repos/{REPO}/{path}"]))


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def verified_run(run_id, source, name, event, jobs, branch="main"):
    run = api(f"actions/runs/{run_id}")
    assert run["head_sha"] == source and run["head_branch"] == branch
    assert run["name"] == name and run["event"] == event
    actual = api(f"actions/runs/{run_id}/jobs?per_page=100")["jobs"]
    for required in jobs:
        matches = [job for job in actual if job["name"] == required]
        assert len(matches) == 1 and matches[0]["conclusion"] == "success", required
    return run["html_url"]


def artifact(run_id, name):
    rows = api(f"actions/runs/{run_id}/artifacts?per_page=100")["artifacts"]
    matches = [row for row in rows if row["name"] == name and not row["expired"]]
    assert len(matches) == 1, name
    item = matches[0]
    assert 0 < item["size_in_bytes"] < 1024 * MIB
    path = ROOT / f"native-artifact-{item['id']}.zip"
    with path.open("wb") as output:
        subprocess.run(
            ["gh", "api", f"repos/{REPO}/actions/artifacts/{item['id']}/zip"],
            stdout=output, check=True,
        )
    assert path.stat().st_size == item["size_in_bytes"], name
    assert "sha256:" + digest(path) == item["digest"], name
    return path, {key: item[key] for key in ("id", "name", "digest", "size_in_bytes")}


def regular_members(archive):
    files = [entry for entry in archive.infolist() if not entry.is_dir()]
    assert len(files) <= 8192 and sum(entry.file_size for entry in files) <= 1024 * MIB
    assert len({entry.filename.casefold() for entry in files}) == len(files)
    for entry in files:
        name = entry.filename
        assert not name.startswith("/") and "\\" not in name and ":" not in name
        assert all(part not in ("", ".", "..") for part in name.split("/"))
        assert not stat.S_ISLNK(entry.external_attr >> 16)
    return files


def check_pe(path, data, arch):
    name = PurePosixPath(path).name.lower()
    offset = struct.unpack_from("<I", data, 0x3c)[0]
    assert data[:2] == b"MZ" and data[offset:offset+4] == b"PE\0\0", path
    nominal = {"X64": 0x8664, "ARM64": 0xaa64}[arch]
    allowed = {nominal}
    if arch == "ARM64" and name in CRT:
        allowed.add(0xa64e)
    assert struct.unpack_from("<H", data, offset + 4)[0] in allowed, path


def windows_package(native_run, arch, tag, artifacts):
    path, info = artifact(native_run, f"serein-Windows-{arch}-unsigned")
    artifacts.append(info)
    layout = {"X64": "media-v5", "ARM64": "media-v6"}[arch]
    archive = OUT / f"serein-{tag}-Windows-{arch}-{layout}.zip"
    files = []
    with zipfile.ZipFile(path) as incoming, zipfile.ZipFile(
        archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6, allowZip64=False,
    ) as outgoing:
        for entry in regular_members(incoming):
            if not entry.filename.startswith("dist/"):
                continue
            name = entry.filename.removeprefix("dist/")
            data = incoming.read(entry)
            if name.lower().endswith((".dll", ".exe")):
                check_pe(name, data, arch)
            target = zipfile.ZipInfo(name, entry.date_time)
            target.compress_type = zipfile.ZIP_DEFLATED
            target.external_attr = (stat.S_IFREG | 0o644) << 16
            outgoing.writestr(target, data, compresslevel=6)
            files.append({"path": name, "bytes": len(data),
                          "sha256": hashlib.sha256(data).hexdigest()})
    subprocess.run(["python3", "packaging/windows/check_update_archive.py", archive], check=True)
    return files


def installer_package(installer_run, arch, source, version, artifacts):
    path, info = artifact(installer_run, f"NuSerein-Windows-{arch}-installer")
    artifacts.append(info)
    name = f"NuSerein-{version}-{source[:7]}-Windows-{arch}-Setup.exe"
    with zipfile.ZipFile(path) as incoming:
        regular_members(incoming)
        sums = incoming.read("SHA256SUMS.txt").decode("utf-8-sig").split()
        assert len(sums) == 2 and sums[1] == name
        with incoming.open(name) as stream, (OUT / name).open("wb") as output:
            shutil.copyfileobj(stream, output)
    assert digest(OUT / name) == sums[0], name


def linux_package(native_run, version, artifacts):
    path, info = artifact(native_run, "serein-Linux-X64-unsigned")
    artifacts.append(info)
    expected_version = version.replace("-", "~", 1) + "-1"
    name = f"serein_{expected_version}_amd64.deb"
    with zipfile.ZipFile(path) as incoming:
        packages = [entry for entry in regular_members(incoming)
                    if entry.filename in (name, "dist/" + name)]
        assert len(packages) == 1
        output = OUT / name.replace("~", ".")
        with incoming.open(packages[0]) as stream, output.open("wb") as target:
            shutil.copyfileobj(stream, target)
    for name, value in [("Package", "serein"), ("Architecture", "amd64"),
                        ("Version", expected_version)]:
        assert subprocess.check_output(["dpkg-deb", "-f", output, name], text=True).strip() == value
    installed_bytes = 0
    executable = None
    with subprocess.Popen(["dpkg-deb", "--fsys-tarfile", output], stdout=subprocess.PIPE) as proc:
        with tarfile.open(fileobj=proc.stdout, mode="r|") as archive:
            for entry in archive:
                if entry.isfile():
                    installed_bytes += entry.size
                if entry.name.removeprefix("./") == "usr/bin/serein" and entry.isfile():
                    checksum = hashlib.sha256()
                    with archive.extractfile(entry) as stream:
                        while chunk := stream.read(MIB):
                            checksum.update(chunk)
                    executable = {"bytes": entry.size, "sha256": checksum.hexdigest()}
        proc.stdout.close()
        assert proc.wait() == 0
    assert executable, output.name
    return {"installed_bytes": installed_bytes, "executable": executable, "asset": output.name}


def matching_assets(release, expected):
    names = set()
    for asset in release["assets"]:
        name = asset["name"]
        assert name not in names and name in expected
        assert asset["state"] == "uploaded"
        assert asset["size"] == (OUT / name).stat().st_size
        assert asset["digest"] == "sha256:" + digest(OUT / name)
        names.add(name)
    return names


def main():
    assert os.environ["GITHUB_REPOSITORY"] == REPO
    plan = json.loads((ROOT / ".github/release/verified-nightly-plan.json").read_text())
    source = plan["source"]
    assert re.fullmatch(r"[0-9a-f]{40}", source) and source == os.environ["SOURCE_COMMIT"]
    native_run = plan["native_run"]
    assert native_run == int(os.environ["SOURCE_RUN"])
    installer_run = plan["installer_run"]
    installer_workflow_commit = plan["installer_workflow_commit"]
    installer_branch = plan["installer_branch"]
    assert re.fullmatch(r"[0-9a-f]{40}", installer_workflow_commit)
    for value in (native_run, installer_run, plan["release_id"]):
        assert isinstance(value, int) and value > 0
    with (ROOT / "Cargo.toml").open("rb") as manifest:
        version = tomllib.load(manifest)["workspace"]["package"]["version"]
    tag = "v" + version
    assert tag == plan["tag"] and re.fullmatch(r"v\d+\.\d+\.\d+-nightly\.\d{8}\.\d+", tag)
    architectures = plan["windows_architectures"]
    assert architectures in (["X64"], ["X64", "ARM64"])
    native_jobs = ["native (ubuntu-latest)", "native (windows-latest)"]
    if "ARM64" in architectures:
        native_jobs.append("native (windows-11-arm)")
    native_url = verified_run(native_run, source, "native-client", "push", native_jobs)
    installer_url = verified_run(installer_run, installer_workflow_commit, "Windows installers from verified binaries",
                                 "workflow_dispatch", [f"installer ({arch})" for arch in architectures],
                                 branch=installer_branch)
    release_path = f"releases/{plan['release_id']}"
    release = api(release_path)
    assert release["draft"] and release["prerelease"] and release["tag_name"] == tag
    assert release["target_commitish"] == source

    OUT.mkdir()
    artifacts = []
    windows = {}
    for arch in architectures:
        windows[arch] = windows_package(native_run, arch, tag, artifacts)
        installer_package(installer_run, arch, source, version, artifacts)
    linux = linux_package(native_run, version, artifacts)
    report = {
        "version": version, "source": source, "native_run": native_run, "native_workflow": native_url,
        "installer_run": installer_run, "installer_workflow": installer_url,
        "installer_workflow_commit": installer_workflow_commit,
        "installer_branch": installer_branch,
        "publication_run": int(os.environ["GITHUB_RUN_ID"]),
        "publication_workflow_commit": os.environ["GITHUB_SHA"],
        "artifacts": artifacts, "windows_payloads": windows, "linux": linux,
        "assets": [{"name": asset.name, "bytes": asset.stat().st_size, "sha256": digest(asset)}
                   for asset in sorted(OUT.iterdir())],
    }
    (OUT / "BUILDINFO.json").write_text(json.dumps(report, indent=2) + "\n")
    (OUT / "SHA256SUMS.txt").write_text("".join(
        f"{digest(asset)}  {asset.name}\n" for asset in sorted(OUT.iterdir())))
    expected = {asset.name for asset in OUT.iterdir()}
    existing = matching_assets(release, expected)
    if expected - existing:
        subprocess.run(["gh", "release", "upload", tag, "-R", REPO,
                        *[str(OUT / name) for name in sorted(expected - existing)]], check=True)
    uploaded = api(release_path)
    assert uploaded["draft"] and matching_assets(uploaded, expected) == expected
    print(f"Verified {len(expected)} uploaded draft assets for {source}")


if __name__ == "__main__":
    main()
