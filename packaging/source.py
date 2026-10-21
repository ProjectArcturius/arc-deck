"""Archive tracked working files plus packaging additions, excluding local build/config data."""
import argparse
import hashlib
from pathlib import Path
import subprocess
import tarfile

parser = argparse.ArgumentParser()
parser.add_argument('--out', type=Path, default=Path('target/dist'))
args = parser.parse_args()
root = Path(__file__).resolve().parent.parent
out = args.out.resolve()
out.mkdir(parents=True, exist_ok=True)
version = '1.0.0'
names = set(subprocess.check_output(['git', 'ls-files', '-z'], cwd=root).decode('utf-8').split('\0'))
# Explicit additions only: never scoop up an untracked user's config, key or task state.
for folder in ['packaging', 'docs']:
    names.update(p.relative_to(root).as_posix() for p in (root / folder).rglob('*') if p.is_file() and '__pycache__' not in p.parts)
names.add('crates/arc-deck-dsh/src/remote/daily.rs')
archive = out / f'saiku-{version}-source.tar.gz'
with tarfile.open(archive, 'w:gz') as tar:
    for name in sorted(names - {''}):
        path = root / name
        if not path.is_file():
            raise RuntimeError(f'tracked source is missing: {name}')
        info = tar.gettarinfo(str(path), arcname=f'saiku-{version}/{name}')
        info.uid = info.gid = 0
        info.uname = info.gname = ''
        info.mode = 0o755 if path.suffix == '.sh' else 0o644
        with path.open('rb') as stream:
            tar.addfile(info, stream)
sha = hashlib.sha256(archive.read_bytes()).hexdigest()
aur = out / 'saiku-aur'
aur.mkdir(exist_ok=True)
(aur / 'PKGBUILD').write_text((root / 'packaging/aur/PKGBUILD.in').read_text(encoding='utf-8').replace('@SOURCE_SHA256@', sha), encoding='utf-8', newline='\n')
(aur / '.SRCINFO').write_text((root / 'packaging/aur/.SRCINFO.in').read_text(encoding='utf-8').replace('@SOURCE_SHA256@', sha), encoding='utf-8', newline='\n')
with tarfile.open(out / f'saiku-{version}-aur.tar.gz', 'w:gz') as tar:
    tar.add(aur / 'PKGBUILD', arcname='saiku-aur/PKGBUILD')
    tar.add(aur / '.SRCINFO', arcname='saiku-aur/.SRCINFO')
aur_archive = out / f'saiku-{version}-aur.tar.gz'
aur_sha = hashlib.sha256(aur_archive.read_bytes()).hexdigest()
(out / 'SHA256SUMS-source.txt').write_text(f'{sha}  {archive.name}\n{aur_sha}  {aur_archive.name}\n', encoding='ascii')
print(f'{archive.name} sha256={sha}')
