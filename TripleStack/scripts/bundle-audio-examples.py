"""Snapshot named presets into the application's compressed example assets."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import zipfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('source', type=Path)
parser.add_argument('--output', type=Path, default=Path(__file__).resolve().parent.parent / 'assets')
args = parser.parse_args()
presets, blobs = [], {}
for manifest in sorted(args.source.glob('*/hitsound-preset.json')):
    folder = manifest.parent
    data = json.loads(manifest.read_text(encoding='utf-8-sig'))
    assert data['version'] == 1
    files = []
    for relative in data['files']:
        relative = relative.replace('\\', '/')
        path = PurePosixPath(relative)
        assert not path.is_absolute() and all(p not in ('..', '.') for p in path.parts)
        source = folder / relative
        assert source.resolve().is_relative_to(folder.resolve())
        content = source.read_bytes()
        digest = hashlib.sha256(content).hexdigest()
        blobs.setdefault(digest, content)
        files.append({'path': relative, 'hash': digest})
    presets.append({'id': folder.name, 'name': data['name'], 'files': files})
assert presets
catalog = json.dumps({'version': 1, 'presets': presets}, ensure_ascii=False, indent=2).encode('utf-8')
args.output.mkdir(parents=True, exist_ok=True)
(args.output / 'audio-examples.json').write_bytes(catalog)
with zipfile.ZipFile(args.output / 'audio-examples.zip', 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
    for name, content in [('index.json', catalog)] + [(f'blobs/{key}', blobs[key]) for key in sorted(blobs)]:
        info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
        info.compress_type = zipfile.ZIP_DEFLATED
        archive.writestr(info, content)
print(f'{len(presets)} examples, {sum(len(p["files"]) for p in presets)} sounds, {len(blobs)} unique files')
print(f'Archive: {(args.output / "audio-examples.zip").stat().st_size:,} bytes')
