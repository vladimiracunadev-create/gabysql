"""Genera el set de iconos requeridos por Tauri 1.6 a partir de un
diseño vectorial básico (un brand mark "▦" sobre un gradient azul,
alineado con la paleta GitHub-style del modeler).

Uso:
    cd desktop/gabymodeler
    python generate_icons.py

Salida: src-tauri/icons/{32x32.png, 128x128.png, 128x128@2x.png,
icon.ico, icon.icns (placeholder), Square*Logo*.png}.

Requiere: Pillow (`pip install Pillow`).
"""
from PIL import Image, ImageDraw, ImageFont, ImageFilter
from pathlib import Path

OUT = Path(__file__).parent / "src-tauri" / "icons"
OUT.mkdir(parents=True, exist_ok=True)

# Paleta — alineada con --bg-0 / --accent / --accent-2 del modeler.
BG_TOP    = (10, 14, 20, 255)      # #0a0e14
ACCENT    = (88, 166, 255, 255)    # #58a6ff
ACCENT_2  = (31, 111, 235, 255)    # #1f6feb
WHITE     = (255, 255, 255, 240)


def draw_master(size: int) -> Image.Image:
    """Render el icono a un tamaño dado, con bordes redondeados,
    gradient diagonal azul y el brand mark blanco encima."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    # 1) Fondo con bordes redondeados (no usamos rounded_rectangle
    # de PIL porque queda con borde duro — hacemos un sample
    # manual sobre una mask para anti-alias).
    mask = Image.new("L", (size * 2, size * 2), 0)
    mdraw = ImageDraw.Draw(mask)
    radius = int(size * 0.22 * 2)
    mdraw.rounded_rectangle([(0, 0), (size * 2, size * 2)], radius=radius, fill=255)
    mask = mask.resize((size, size), Image.LANCZOS)

    # 2) Gradient diagonal sobre la mask
    grad = Image.new("RGBA", (size, size), ACCENT_2)
    gdraw = ImageDraw.Draw(grad)
    for i in range(size):
        ratio = i / size
        r = int(ACCENT_2[0] + (ACCENT[0] - ACCENT_2[0]) * ratio)
        g = int(ACCENT_2[1] + (ACCENT[1] - ACCENT_2[1]) * ratio)
        b = int(ACCENT_2[2] + (ACCENT[2] - ACCENT_2[2]) * ratio)
        gdraw.line([(0, i), (size, i)], fill=(r, g, b, 255))

    base = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    base.paste(grad, (0, 0), mask)

    # 3) Brand mark: un grid 2x2 minimalista — 4 cuadrados blancos
    # con un cuadrado central destacado, evoca el ▦ del modeler.
    # Usamos rectángulos sobre la base.
    bdraw = ImageDraw.Draw(base)
    pad   = int(size * 0.27)
    cell  = int(size * 0.18)
    gap   = int(size * 0.04)
    x0 = pad
    y0 = pad
    # Grid 2x2
    for r in range(2):
        for c in range(2):
            x = x0 + c * (cell + gap)
            y = y0 + r * (cell + gap)
            corner = int(cell * 0.18)
            bdraw.rounded_rectangle(
                [(x, y), (x + cell, y + cell)],
                radius=corner,
                fill=WHITE,
            )

    # 4) Sombra interior suave para que el mark "flote"
    shadow = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    sdraw = ImageDraw.Draw(shadow)
    sdraw.rounded_rectangle(
        [(int(size * 0.04), int(size * 0.04)),
         (int(size * 0.96), int(size * 0.96))],
        radius=int(size * 0.19),
        outline=(255, 255, 255, 25),
        width=max(2, size // 96),
    )
    base = Image.alpha_composite(base, shadow)

    return base


def main():
    print(f"OUT = {OUT}")
    # Master a 1024 → resizes
    master = draw_master(1024)
    master.save(OUT / "icon-source.png", "PNG")
    print("OK icon-source.png (1024x1024)")

    sizes = [
        ("32x32.png",       32),
        ("128x128.png",     128),
        ("128x128@2x.png",  256),
    ]
    for name, sz in sizes:
        master.resize((sz, sz), Image.LANCZOS).save(OUT / name, "PNG")
        print(f"OK {name}")

    # Logos Microsoft Store (Square*Logo)
    msstore = [
        ("StoreLogo.png",          50),
        ("Square30x30Logo.png",    30),
        ("Square44x44Logo.png",    44),
        ("Square71x71Logo.png",    71),
        ("Square89x89Logo.png",    89),
        ("Square107x107Logo.png", 107),
        ("Square142x142Logo.png", 142),
        ("Square150x150Logo.png", 150),
        ("Square284x284Logo.png", 284),
        ("Square310x310Logo.png", 310),
    ]
    for name, sz in msstore:
        master.resize((sz, sz), Image.LANCZOS).save(OUT / name, "PNG")
        print(f"OK {name}")

    # ICO (Windows) — multi-resolución
    ico_sizes = [(s, s) for s in (16, 24, 32, 48, 64, 96, 128, 256)]
    master.save(
        OUT / "icon.ico",
        format="ICO",
        sizes=ico_sizes,
    )
    print(f"OK icon.ico (multi-res: {[s[0] for s in ico_sizes]})")

    # ICNS (macOS) — Pillow no genera ICNS directo en todas las versiones,
    # así que dejamos un PNG con extension renombrada como placeholder.
    # `cargo tauri build --bundles dmg` espera icon.icns para bundle Mac;
    # en Windows-only se ignora.
    master.resize((512, 512), Image.LANCZOS).save(OUT / "icon.icns.png", "PNG")
    print("OK icon.icns.png (placeholder; macOS: generar con iconutil)")


if __name__ == "__main__":
    main()
