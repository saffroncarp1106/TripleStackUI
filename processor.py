import shutil, re
from pathlib import Path
from PIL import Image
from ini_utils import update_skin_ini
from image_utils import stack_overlay, trim_transparency
from resources import resource_path, MissingAssetError


def delete_2x_assets(skin_dir: Path):
    for f in skin_dir.iterdir():
        if '@2x' in f.name.lower():
            try:
                f.unlink()
            except FileNotFoundError:
                pass

def delete_follow_point_animation(skin_dir: Path):
    for f in skin_dir.iterdir():
        name = f.stem.lower()  # without extension
        if any(name == f'followpoint-{i}' for i in range(10)):
            try:
                f.unlink()
            except FileNotFoundError:
                pass

def copy_replace_follow_Point(skin_dir: Path):
    # Copy followpoint.png from this code directory into the skin folder
    # creating followpoint-0.png .. followpoint-9.png. Do not delete any
    # existing files first; shutil.copy2 will overwrite them.
    script_src = resource_path("followpoint.png")

    # If the script's followpoint exists, copy it into the skin folder as the
    # canonical `followpoint.png`. Otherwise fall back to an existing skin copy.
    if script_src.exists():
        try:
            shutil.copy2(script_src, skin_dir / "followpoint.png")
        except Exception:
            # ignore and continue to best-effort frame creation
            pass

    base = skin_dir / "followpoint.png"
    if not base.exists():
        return

    # Create numbered frames from the canonical followpoint.png in the skin
    for i in range(10):
        dst = skin_dir / f"followpoint-{i}.png"
        try:
            shutil.copy2(base, dst)
        except Exception:
            # best-effort copy; ignore single-file failures
            pass

def triple_stack_skin(skin_dir: Path, overlay_file="hitcircleoverlay.png", force_res=None, use_hitcircle_overlay=False, overlay_on_top=False):
    provided_f = resource_path("hitcircle.png")
    hitcircle_path = skin_dir / "hitcircle.png"
    overlay_img = None
    # Composite logic matches UI preview
    if overlay_on_top:
        # If 'Use existing hitcircle + Rafis circle' is ON, always composite both
        if provided_f.exists() and hitcircle_path.exists():
            base_img = Image.open(hitcircle_path).convert('RGBA')
            overlay_img2 = Image.open(provided_f).convert('RGBA')
            if overlay_img2.size != base_img.size:
                overlay_img2 = overlay_img2.resize(base_img.size, Image.LANCZOS)
            overlay_img = base_img.copy()
            overlay_img.alpha_composite(overlay_img2)
        elif provided_f.exists():
            overlay_img = Image.open(provided_f).convert('RGBA')
        elif hitcircle_path.exists():
            overlay_img = Image.open(hitcircle_path).convert('RGBA')
        else:
            raise MissingAssetError(
                f"Neither the bundled hitcircle.png ({provided_f}) nor the skin's "
                f"hitcircle.png ({hitcircle_path}) exists; nothing to overlay."
            )
    elif use_hitcircle_overlay:
        if provided_f.exists():
            overlay_img = Image.open(provided_f).convert('RGBA')
        else:
            raise MissingAssetError(
                f"'Use Rafis hitcircle' is enabled but the bundled hitcircle.png "
                f"was not found at {provided_f}. If this is a packaged build, "
                f'rebuild with: pyinstaller --add-data "hitcircle.png;." ...'
            )
    else:
        if not hitcircle_path.exists():
            raise MissingAssetError(
                f"The selected skin has no hitcircle.png ({hitcircle_path})."
            )
        overlay_img = Image.open(hitcircle_path).convert('RGBA')
    # Copy overlay image to hitcircleoverlay.png
    overlay_path = skin_dir / "hitcircleoverlay.png"
    overlay_img.save(overlay_path)
    target_size = None
    if force_res:
        try:
            w = int(force_res)
            target_size = (w, w)
        except ValueError:
            pass
    for img_path in skin_dir.glob("default-[0-9]*.png"):
        with Image.open(img_path) as base:
            base = base.convert('RGBA')
            # Determine output canvas size
            out_size = target_size if target_size else overlay_img.size
            # Create canvas and center default-# image
            canvas = Image.new('RGBA', out_size, (0, 0, 0, 0))
            base_pos = ((out_size[0] - base.width) // 2, (out_size[1] - base.height) // 2)
            canvas.paste(base, base_pos, base)
            # Resize overlay image to canvas size and overlay
            overlay_resized = overlay_img.resize(out_size, Image.LANCZOS) if overlay_img.size != out_size else overlay_img
            stacked = stack_overlay(canvas, overlay_resized, out_size[0])
            stacked.save(img_path)


def copy_and_process(src: Path, do_copy: bool, *, force_res=None, use_hitcircle_overlay=False, overlay_on_top=False, modify_followpoint: bool = True):
    dst = src
    if do_copy:
        base_name = f"{src.name}@3xStack"
        dst = src.parent / base_name
        i = 1
        while dst.exists():
            dst = src.parent / f"{base_name}({i})"
            i += 1
        shutil.copytree(src, dst)

    delete_2x_assets(dst)
    # Only remove existing followpoint animation files if user requested followpoint modification
    if modify_followpoint:
        delete_follow_point_animation(dst)
        # Optionally recreate numbered followpoint frames from the source followpoint.png
        try:
            copy_replace_follow_Point(dst)
        except Exception:
            # Best-effort: don't fail the entire processing on copy issues
            pass
    hitcircle = dst / "hitcircle.png"
    hitcircleoverlay = dst / "hitcircleoverlay.png"
    if hitcircleoverlay.exists():
        hitcircleoverlay.unlink()
    if hitcircle.exists():
        shutil.copy2(hitcircle, hitcircleoverlay)
    triple_stack_skin(dst, force_res=force_res, use_hitcircle_overlay=use_hitcircle_overlay, overlay_on_top=overlay_on_top)
    update_skin_ini(dst, overlap=int(force_res) if force_res else 160)
    return dst
