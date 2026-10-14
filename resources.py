import sys, os
from pathlib import Path


class MissingAssetError(RuntimeError):
    """Raised when a bundled asset (hitcircle.png, followpoint.png) can't be found."""


def _search_roots():
    """Directories that may hold the bundled assets, most-likely first."""
    roots = []
    # PyInstaller onefile: assets added with --add-data land in the _MEIPASS temp dir
    meipass = getattr(sys, '_MEIPASS', None)
    if meipass:
        roots.append(Path(meipass))
    if getattr(sys, 'frozen', False):
        exe_dir = Path(sys.executable).resolve().parent
        # onedir layout, plus assets dropped next to the .exe by hand
        roots.append(exe_dir / '_internal')
        roots.append(exe_dir)
    # running from source
    roots.append(Path(__file__).resolve().parent)
    roots.append(Path(os.getcwd()))

    seen, out = set(), []
    for r in roots:
        if r not in seen:
            seen.add(r)
            out.append(r)
    return out


def resource_path(name: str) -> Path:
    """Locate a bundled asset. Works from source and from a frozen exe.

    Returns the first existing match; if nothing exists, returns the primary
    candidate so callers can still report a meaningful path.
    """
    roots = _search_roots()
    for root in roots:
        p = root / name
        if p.exists():
            return p
    return roots[0] / name


def require_resource(name: str) -> Path:
    """Like resource_path, but raises instead of failing silently."""
    p = resource_path(name)
    if not p.exists():
        tried = "\n  ".join(str(r) for r in _search_roots())
        raise MissingAssetError(
            f"Required asset '{name}' was not found.\n"
            f"Searched:\n  {tried}\n"
            f"If this is a packaged build, rebuild with:\n"
            f'  pyinstaller --add-data "{name};." ...'
        )
    return p
