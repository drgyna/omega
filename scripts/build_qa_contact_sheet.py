"""Crea una hoja de contacto temporal para revisar visualmente artefactos de QA."""
from __future__ import annotations

import math
import sys
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont


def font(size: int):
    for candidate in ["/System/Library/Fonts/Supplemental/Arial.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"]:
        if Path(candidate).is_file():
            return ImageFont.truetype(candidate, size)
    return ImageFont.load_default()


def main() -> None:
    image_paths = [Path(item) for item in sys.argv[2:]]
    output = Path(sys.argv[1])
    width, height, label = 360, 470, 42
    columns = 4
    rows = math.ceil(len(image_paths) / columns)
    canvas = Image.new("RGB", (columns * width, rows * height), "white")
    draw = ImageDraw.Draw(canvas)
    for index, source in enumerate(image_paths):
        image = Image.open(source).convert("RGB")
        image.thumbnail((width - 18, height - label - 18))
        x, y = (index % columns) * width, (index // columns) * height
        canvas.paste(image, (x + (width - image.width) // 2, y + 8))
        draw.text((x + 8, y + height - label + 8), source.stem[:42], fill="#111111", font=font(15))
    output.parent.mkdir(parents=True, exist_ok=True)
    canvas.save(output)
    print(output)


if __name__ == "__main__":
    main()
