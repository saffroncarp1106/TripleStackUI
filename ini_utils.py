from pathlib import Path
import re

def update_skin_ini(skin_dir: Path, overlap: int = 160):
    ini = skin_dir / "skin.ini"
    if not ini.exists():
        return
    try:
        text = ini.read_text(encoding='utf-16')
    except UnicodeError:
        text = ini.read_text(encoding='latin-1')
    overlap_re = re.compile(r'(?mi)^\s*HitCircleOverlap\s*:.*\n')
    text = overlap_re.sub("", text)
    if re.search(r'(?mi)^\[Fonts\]', text):
        text = re.sub(
            r'(?mi)(^\[Fonts\]\s*$)',
            rf"\1\nHitCircleOverlap: {overlap}",
            text,
            count=1,
        )
    else:
        if not text.endswith('\n'):
            text += '\n'
        text += f"[Fonts]\nHitCircleOverlap: {overlap}\n"
    try:
        ini.write_text(text, encoding='utf-16')
    except UnicodeError:
        ini.write_text(text, encoding='latin-1')
