"""One-off draft asset upload for the already merged and tested November release."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import struct
import subprocess
import zipfile

REPO = "MCShotty/NuSerein"
SOURCE = "5e21524c0cbfc256edc7772bf58175fc57960829"
NATIVE_RUN = 43933415531
TAG = "v0.1.1-nightly.20261116.1"
ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "release-assets"


def gh(*args):
    return subprocess.check_output(["gh", *map(str, args)])


def api(path):
    return json.loads(gh("api", f"repos/{REPO}/{path}"))


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def artifact(run, name):
    matches = [a for a in api(f"actions/runs/{run}/artifacts?per_page=100")["artifacts"]
               if a["name"] == name and not a["expired"]]
    assert len(matches) == 1, name
    item = matches[0]
    assert item["size_in_bytes"] < 1024**3
    path = ROOT / f"artifact-{item['id']}.zip"
    with path.open("wb") as output:
        subprocess.run(["gh", "api", f"repos/{REPO}/actions/artifacts/{item['id']}/zip"],
                       stdout=output, check=True)
    assert "sha256:" + digest(path) == item["digest"], name
    return path, {k: item[k] for k in ("id", "name", "digest", "size_in_bytes")}


def regular_members(archive):
    members = [i for i in archive.infolist() if not i.is_dir()]
    assert len(members) <= 8192 and sum(i.file_size for i in members) <= 1024**3
    assert len({i.filename for i in members}) == len(members)
    for entry in members:
        assert not entry.filename.startswith("/") and "\\" not in entry.filename
        assert all(p not in ("", ".", "..") for p in entry.filename.split("/"))
        assert not stat.S_ISLNK(entry.external_attr >> 16)
    return members


def main():
    assert os.environ["GITHUB_REPOSITORY"] == REPO
    assert os.environ["SOURCE_COMMIT"] == SOURCE
    assert int(os.environ["SOURCE_RUN"]) == NATIVE_RUN
    installer_run = 43951006518
    installer_source = api(f"actions/runs/{installer_run}")
    assert installer_source["head_sha"] == "7a864e1df69d9527a0c11c645f5b7196e327cd03"
    run = api(f"actions/runs/{NATIVE_RUN}")
    assert run["head_sha"] == SOURCE and run["name"] == "native-client"
    jobs = api(f"actions/runs/{NATIVE_RUN}/jobs?per_page=100")["jobs"]
    for name in ("native (windows-latest)", "native (ubuntu-latest)"):
        matches = [j for j in jobs if j["name"] == name]
        assert len(matches) == 1 and matches[0]["conclusion"] == "success", name
    installer_jobs = api(f"actions/runs/{installer_run}/jobs?per_page=100")["jobs"]
    assert any(j["name"] == "installer (X64)" and j["conclusion"] == "success"
               for j in installer_jobs)
    # GitHub's tag lookup excludes unpublished drafts; address the verified draft by ID.
    release = api("releases/447562482")
    assert release["tag_name"] == TAG
    assert release["draft"] and release["prerelease"] and not release["assets"]
    assert release["target_commitish"] == SOURCE
    OUT.mkdir()
    windows_path, windows_info = artifact(NATIVE_RUN, "serein-Windows-X64-unsigned")
    windows_name = f"serein-{TAG}-Windows-X64-media-v5.zip"
    payload = []
    with zipfile.ZipFile(windows_path) as source, zipfile.ZipFile(
        OUT / windows_name, "w", compression=zipfile.ZIP_DEFLATED,
        compresslevel=6, allowZip64=False
    ) as dest:
        for entry in regular_members(source):
            if not entry.filename.startswith("dist/"):
                continue
            name = entry.filename.removeprefix("dist/")
            data = source.read(entry)
            if name.lower().endswith((".dll", ".exe")):
                assert data[:2] == b"MZ", name
                offset = struct.unpack_from("<I", data, 0x3C)[0]
                assert data[offset:offset+4] == b"PE\0\0", name
                assert struct.unpack_from("<H", data, offset+4)[0] == 0x8664, name
            info = zipfile.ZipInfo(name, entry.date_time)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = (stat.S_IFREG | 0o644) << 16
            with dest.open(info, "w") as stream:
                stream.write(data)
            payload.append({"path": name, "bytes": len(data),
                            "sha256": hashlib.sha256(data).hexdigest()})
    spec = importlib.util.spec_from_file_location(
        "archive_check", ROOT / "packaging/windows/check_update_archive.py")
    checker = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(checker)
    checker.check_archive(OUT / windows_name)
    linux_path, linux_info = artifact(NATIVE_RUN, "serein-Linux-X64-unsigned")
    linux_name = "serein_0.1.1.nightly.20261116.1-1_amd64.deb"
    with zipfile.ZipFile(linux_path) as archive:
        names = [i.filename for i in regular_members(archive) if i.filename.endswith(".deb")]
        assert len(names) == 1
        (OUT / linux_name).write_bytes(archive.read(names[0]))
    assert digest(OUT / linux_name) == "4f1bc4e8131fd82f252936e654ff3b54daa76b9c99d26b9a1de2bf6cbd95aaf1"
    installer_path, installer_info = artifact(installer_run, "NuSerein-Windows-X64-installer")
    installer_name = "NuSerein-0.1.1-nightly.20261116.1-5e21524-Windows-X64-Setup.exe"
    with zipfile.ZipFile(installer_path) as archive:
        regular_members(archive)
        sums = archive.read("SHA256SUMS.txt").decode("utf-8-sig").split()
        assert sums[1] == installer_name
        (OUT / installer_name).write_bytes(archive.read(installer_name))
        assert digest(OUT / installer_name) == sums[0]
    report = json.loads((ROOT / ".github/release/verified-release-evidence.json").read_text())
    report.update(source=SOURCE, native_run=NATIVE_RUN, installer_run=installer_run,
                  publication_run=int(os.environ["GITHUB_RUN_ID"]),
                  publication_workflow_commit=os.environ["GITHUB_SHA"],
                  artifacts=[windows_info, linux_info, installer_info],
                  windows_payload=payload,
                  assets=[{"name": p.name, "bytes": p.stat().st_size, "sha256": digest(p)}
                          for p in sorted(OUT.iterdir())])
    (OUT / "BUILDINFO.json").write_text(json.dumps(report, indent=2) + "\n")
    (OUT / "SHA256SUMS.txt").write_text("".join(
        f"{digest(p)}  {p.name}\n" for p in sorted(OUT.iterdir())))
    subprocess.run(["gh", "release", "upload", TAG, "-R", REPO,
                    *map(str, sorted(OUT.iterdir()))], check=True)
    uploaded = api(f"releases/{release['id']}")
    assert uploaded["draft"], "Publication is a separate explicit step"
    for asset in uploaded["assets"]:
        path = OUT / asset["name"]
        assert asset["digest"] == "sha256:" + digest(path)
        assert asset["size"] == path.stat().st_size
    assert len(uploaded["assets"]) == 5
    print("Draft contains five verified assets; not published.")


if __name__ == "__main__":
    main()
