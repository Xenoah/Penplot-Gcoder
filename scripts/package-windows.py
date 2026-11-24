"""Package already-built native Studio + CLI with docs, examples and dependency notices.

Developer convenience only; the two executables do not need Python or Cargo.
Usage: python scripts/package-windows.py x86_64-pc-windows-msvc target/release output.zip
Run from the repository root, after a locked build for the matching target.
"""
import json
from pathlib import Path
import subprocess
import sys
import tomllib
import zipfile


def notices(target):
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--all-features", "--format-version", "1",
        "--filter-platform", target,
    ]))
    texts = [
        "Penplot Studio | Third-party components",
        "Cargo package notices, declarations and bundled font license texts follow.",
        "The metadata inventory can include optional or build-time components.",
    ]
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if not package["source"]:
            continue
        directory = Path(package["manifest_path"]).parent
        texts.extend(["", "=" * 72, f'{package["name"]} {package["version"]}',
                      f'License declaration: {package["license"] or "(see license file)"}',
                      f'Authors: {", ".join(package.get("authors", []))}',
                      f'Source: {package.get("repository") or package.get("homepage") or package["source"]}'])
        declared_file = package.get("license_file")
        for file in sorted(directory.rglob("*")):
            if not file.is_file():
                continue
            name = file.name.upper()
            rel = file.relative_to(directory)
            if (name.startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "OFL.", "UFL."))
                    or (package["name"] == "epaint_default_fonts" and file.suffix == ".txt")
                    or str(rel) == declared_file):
                try:
                    txt = file.read_text(encoding="utf-8")
                except UnicodeDecodeError:
                    continue
                texts.extend([str(rel), txt.strip()])
    root = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
    licenses = root / "share" / "doc" / "rust" / "licenses"
    if licenses.is_dir():
        texts.append("\nRust toolchain license texts")
        for file in sorted(licenses.glob("*")):
            if file.is_file():
                texts.extend([file.name, file.read_text(encoding="utf-8")])
    return "\n".join(texts) + "\n"


def main():
    if len(sys.argv) != 4:
        raise SystemExit(__doc__)
    target, binary_dir, output = sys.argv[1:]
    if target not in {"x86_64-pc-windows-msvc", "x86_64-pc-windows-gnu"}:
        raise SystemExit("Expected Windows x64 MSVC or GNU target")
    files = {name: Path(binary_dir) / name for name in ("penplot-studio.exe", "penplot.exe")}
    for name in ("README.md", "docs/K9.md", "docs/MACHINE-PRESETS.md",
                 "examples/alignment-card.svg", "examples/calibration.dxf"):
        files[name] = Path(name)
    for name in ("docs/studio-preview.png", "docs/studio-lifts.png"):
        if Path(name).exists():
            files[name] = Path(name)
    docs = {name: file.read_bytes() for name, file in files.items()}
    version = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    docs["VERSION.txt"] = f"Penplot-Gcoder Studio v{version}\nTarget: {target}\n".encode("utf-8")
    docs["THIRD-PARTY-NOTICES.txt"] = notices(target).encode()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
        for name, data in sorted(docs.items()):
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            bundle.writestr(info, data)


if __name__ == "__main__":
    main()
