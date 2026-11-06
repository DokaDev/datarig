"""The repository's social preview: the still of social.tape under a band with the name and
the tagline, 1280x640 (GitHub's size). Used by render.sh; needs Pillow.

    python3 social.py <still.png> <out.png> <bold font file> <regular font file>
"""

import sys

from PIL import Image, ImageDraw, ImageFont

WIDTH, HEIGHT, BAND = 1280, 640, 104
BACKGROUND = (0x1A, 0x1B, 0x26)
TITLE = (0xC0, 0xCA, 0xF5)
TEXT = (0x9A, 0xA5, 0xCE)
TAGLINE = "A full-featured database client that lives in your terminal: light, fast, and safe by default."


def main(still_path: str, out_path: str, bold: str, regular: str) -> None:
    out = Image.new("RGB", (WIDTH, HEIGHT), BACKGROUND)
    still = Image.open(still_path).convert("RGB")
    still.thumbnail((WIDTH, HEIGHT - BAND), Image.Resampling.LANCZOS)
    out.paste(still, ((WIDTH - still.width) // 2, BAND + (HEIGHT - BAND - still.height) // 2))
    draw = ImageDraw.Draw(out)
    draw.text((40, 16), "datarig", font=ImageFont.truetype(bold, 46), fill=TITLE)
    draw.text((40, 72), TAGLINE, font=ImageFont.truetype(regular, 19), fill=TEXT)
    out.save(out_path, optimize=True)


if __name__ == "__main__":
    main(*sys.argv[1:5])
