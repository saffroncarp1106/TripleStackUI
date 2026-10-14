# -*- mode: python ; coding: utf-8 -*-
# Build with:  pyinstaller TripleStack.spec
#
# The datas list below is the whole point of this file: without it the exe
# ships without hitcircle.png / followpoint.png and the overlay step no-ops.

a = Analysis(
    ['main.py'],
    pathex=[],
    binaries=[],
    datas=[
        ('hitcircle.png', '.'),
        ('followpoint.png', '.'),
    ],
    hiddenimports=[],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[],
    noarchive=False,
)
pyz = PYZ(a.pure)

exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.datas,
    [],
    name='TripleStackV0.2.2',
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    upx_exclude=[],
    runtime_tmpdir=None,
    console=False,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
    icon='icon.ico',
)
