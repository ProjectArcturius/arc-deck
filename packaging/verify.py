"""Verify the release's checksums, source/recipe agreement, notices and Windows DLL imports."""
import hashlib
import json
from pathlib import Path
import struct
import sys
import tarfile
import zipfile

dist = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/dist')
for sums in ['SHA256SUMS-windows.txt', 'SHA256SUMS-source.txt']:
    for line in (dist / sums).read_text(encoding='ascii').splitlines():
        expected, name = line.split('  ', 1)
        assert Path(name).name == name
        assert hashlib.sha256((dist / name).read_bytes()).hexdigest() == expected, name
source = dist / 'arc-deck-1.0.0-source.tar.gz'
source_sha = hashlib.sha256(source.read_bytes()).hexdigest()
with tarfile.open(source) as archive:
    names = archive.getnames()
    assert all(n.startswith('arc-deck-1.0.0/') and '..' not in Path(n).parts for n in names)
    assert not any(n.split('/')[1] in {'REFS', '.git', 'target', '.cargo'} for n in names)
    for required in ['Cargo.toml', 'Cargo.lock', 'agents/echo.md', 'prompts/echo-style.md', 'docs/LINUX_RELEASE.md', 'packaging/debian/build.sh', 'crates/arc-deck-dsh/src/remote/daily.rs']:
        assert f'arc-deck-1.0.0/{required}' in names, required
with tarfile.open(dist / 'arc-deck-1.0.0-aur.tar.gz') as archive:
    for name in ['arc-deck-aur/PKGBUILD', 'arc-deck-aur/.SRCINFO']:
        recipe = archive.extractfile(name).read().decode('utf-8')
        assert source_sha in recipe and '@SOURCE_SHA256@' not in recipe
with zipfile.ZipFile(dist / 'arcdeck-1.0.0-windows-x86_64.zip') as archive:
    for name in ['LICENSE', 'NOTICE', 'LICENSE-OpenCode', 'INSTALL.md', 'BUILD-INFO.json', 'install.cmd', 'install.ps1', 'uninstall.cmd', 'uninstall.ps1']:
        assert name in archive.namelist(), name
    info = json.loads(archive.read('BUILD-INFO.json').decode('utf-8-sig'))
    assert info['version'] == '1.0.0'
    binary = archive.read('arcdeck.exe')
    if len(sys.argv) > 2:
        assert binary == Path(sys.argv[2]).read_bytes(), 'ZIP executable differs from release binary'

# PE import table: a MinGW-built executable must not require an unshipped MSYS/GCC DLL.
pe = struct.unpack_from('<I', binary, 0x3c)[0]
assert binary[pe:pe + 4] == b'PE\0\0'
machine, sections = struct.unpack_from('<HH', binary, pe + 4)
assert machine == 0x8664, 'expected a Windows x64 executable'
optional_size = struct.unpack_from('<H', binary, pe + 20)[0]
optional = pe + 24
assert struct.unpack_from('<H', binary, optional)[0] == 0x20b
table = optional + optional_size

def offset(rva):
    for i in range(sections):
        virtual_size, virtual_address, raw_size, raw_address = struct.unpack_from('<IIII', binary, table + i * 40 + 8)
        if virtual_address <= rva < virtual_address + max(virtual_size, raw_size):
            return raw_address + rva - virtual_address
    raise ValueError(f'RVA outside sections: {rva:x}')

imports = offset(struct.unpack_from('<I', binary, optional + 112 + 8)[0])
dlls = []
while any(binary[imports:imports + 20]):
    name = offset(struct.unpack_from('<I', binary, imports + 12)[0])
    dlls.append(binary[name:binary.index(b'\0', name)].decode('ascii'))
    imports += 20
assert not any(n.lower().startswith(('libgcc', 'libstdc++', 'libwinpthread', 'msys-')) for n in dlls), dlls
print('Release assets, notices, source/recipe hashes and Windows x64 imports: PASS')
print('DLL imports: ' + ', '.join(dlls))
print('BUILD-INFO: ' + json.dumps(info))
