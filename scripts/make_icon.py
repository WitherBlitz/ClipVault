"""Generate the app's original clipboard icon with no external assets."""
from pathlib import Path
from PIL import Image, ImageDraw

root = Path(__file__).resolve().parents[1]
im = Image.new("RGBA", (256, 256))
d = ImageDraw.Draw(im)
d.rounded_rectangle((23, 37, 233, 248), radius=34, fill="#85e5b8")
d.rounded_rectangle((50, 62, 206, 223), radius=16, fill="#182128")
d.rounded_rectangle((82, 8, 174, 74), radius=18, fill="#eaf8f2")
d.rounded_rectangle((106, 22, 150, 41), radius=8, fill="#182128")
for y, end in [(107, 179), (145, 179), (183, 149)]:
    d.rounded_rectangle((77, y, end, y + 12), radius=6, fill="#eaf8f2")
im.save(root / "assets/clipvault.ico", sizes=[(16,16),(20,20),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])
im.save(root / "assets/clipvault.png")
