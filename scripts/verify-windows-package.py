"""Validate a CI portable Windows archive and its exact source commit. Does not publish."""
from pathlib import Path
import struct
import sys
import tomllib
import zipfile

archive, expected_sha = sys.argv[1:]
version = tomllib.loads(Path('Cargo.toml').read_text(encoding='utf-8'))['package']['version']
with zipfile.ZipFile(archive) as bundle:
    assert bundle.testzip() is None, 'ZIP CRC failure'
    assert bundle.read('SOURCE_COMMIT.txt').decode().strip() == expected_sha
    assert bundle.read('VERSION.txt').decode().splitlines() == [f'Penplot-Gcoder v{version}', 'Target: x86_64-pc-windows-msvc']
    for name in ['penplot.exe', 'penplot-studio.exe']:
        exe = bundle.read(name)
        assert exe[:2] == b'MZ', name
        pe = struct.unpack_from('<I', exe, 0x3c)[0]
        assert exe[pe:pe+4] == b'PE\0\0', name
        assert struct.unpack_from('<H', exe, pe+4)[0] == 0x8664, 'Expected AMD64 PE'
    for name in ['README.md', 'docs/RASTER-COLOR.ja.md', 'docs/PEN-WIDTH.ja.md', 'docs/releases/v0.12.0.md', 'examples/color-card.png', 'examples/clean-scan.png', 'THIRD-PARTY-NOTICES.txt']:
        assert bundle.read(name), f'Empty or missing: {name}'
print(f'Validated v{version} Windows x64 ZIP from {expected_sha}')
