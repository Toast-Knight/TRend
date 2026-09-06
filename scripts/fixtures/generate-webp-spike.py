from pathlib import Path

from PIL import Image, ImageDraw


OUTPUT = Path(__file__).resolve().parents[2] / "tests" / "fixtures" / "media" / "WebP Spike"
OUTPUT.mkdir(parents=True, exist_ok=True)


def scene(width: int, height: int) -> Image.Image:
    image = Image.new("RGB", (width, height))
    pixels = image.load()
    for y in range(height):
        for x in range(width):
            pixels[x, y] = (
                (x * 255) // max(width - 1, 1),
                (y * 255) // max(height - 1, 1),
                ((x * 17) ^ (y * 29)) & 255,
            )
    draw = ImageDraw.Draw(image)
    draw.ellipse(
        (width // 5, height // 6, width * 3 // 5, height * 5 // 6),
        fill=(240, 48, 32),
        outline=(255, 240, 80),
        width=max(1, width // 128),
    )
    return image


scene(2, 2).save(OUTPUT / "WebPTinyLossless.webp", format="WEBP", lossless=True)
scene(640, 360).save(
    OUTPUT / "WebPLossyScene.webp", format="WEBP", quality=78, method=6
)
scene(640, 360).save(
    OUTPUT / "WebPLosslessScene.webp", format="WEBP", lossless=True, method=6
)
scene(2048, 1024).save(
    OUTPUT / "WebPLargeLossy.webp", format="WEBP", quality=82, method=6
)

alpha = scene(640, 360).convert("RGBA")
alpha_pixels = alpha.load()
for y in range(alpha.height):
    for x in range(alpha.width):
        red, green, blue, _ = alpha_pixels[x, y]
        alpha_pixels[x, y] = (red, green, blue, (x * 255) // (alpha.width - 1))
alpha.save(OUTPUT / "WebPAlpha.webp", format="WEBP", quality=80, method=6)

animated_frames = [
    Image.new("RGBA", (32, 32), (255, 0, 0, 255)),
    Image.new("RGBA", (32, 32), (0, 0, 255, 128)),
]
animated_frames[0].save(
    OUTPUT / "WebPAnimatedRejected.webp",
    format="WEBP",
    save_all=True,
    append_images=animated_frames[1:],
    duration=[80, 120],
    loop=0,
    lossless=True,
)

print(f"Generated WebP spike fixtures in {OUTPUT}")