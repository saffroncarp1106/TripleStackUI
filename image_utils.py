from PIL import Image, ImageTk, ImageDraw

def trim_transparency(img: Image.Image) -> Image.Image:
    """Crops fully transparent padding around an image."""
    bbox = img.getbbox()
    return img.crop(bbox) if bbox else img


def stack_overlay(base_img: Image.Image, overlay: Image.Image, offset: int = 160) -> Image.Image:
    #attempt to handle image size variation#
    canvas_size = base_img.size
    canvas = Image.new('RGBA', canvas_size, (0, 0, 0, 0))

    canvas.paste(base_img, (0, 0))

    x = (canvas_size[0] - overlay.width) // 2
    y = (canvas_size[1] - overlay.height) // 2
    canvas.paste(overlay, (x, y), overlay)

    return canvas

def make_pattern(w, h, bg, fg, step):
    """Create a modern gradient triangle pattern button"""
    img = Image.new('RGBA', (w, h), bg)
    draw = ImageDraw.Draw(img)

    # Create triangle pattern with modern styling
    for y in range(0, h, step):
        for x in range(0, w, step):
            draw.polygon([(x, y + step), (x + step // 2, y), (x + step, y + step)], fg)

    # Add subtle border for depth
    draw.rectangle([(0, 0), (w-1, h-1)], outline=fg, width=1)

    return ImageTk.PhotoImage(img)
