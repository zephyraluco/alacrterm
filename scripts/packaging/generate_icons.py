"""Manually derive committed platform icons from assets/app-icon.svg."""
import io
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parents[2]
MACOS_CONTENT_RATIO = 824 / 1024
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)
ICNS_TYPES = {128: b'ic07', 256: b'ic08', 512: b'ic09', 1024: b'ic10'}


def artwork(source):
    import cairosvg
    from PIL import Image

    rendered = cairosvg.svg2png(url=str(source), output_width=1024, output_height=1024)
    with Image.open(io.BytesIO(rendered)) as image:
        image = image.convert('RGBA')
    bounds = image.getchannel('A').getbbox()
    if bounds is None:
        raise ValueError('SVG has no visible artwork')
    # Remove the SVG's existing platform margins before applying each target's layout.
    return image.crop(bounds)


def png_frame(image, size, macos=False):
    from PIL import Image, ImageOps

    extent = round(size * MACOS_CONTENT_RATIO) if macos else size - 2
    content = ImageOps.contain(image, (extent, extent), Image.Resampling.LANCZOS)
    canvas = Image.new('RGBA', (size, size), (0, 0, 0, 0))
    canvas.alpha_composite(content, ((size - content.width) // 2, (size - content.height) // 2))
    output = io.BytesIO()
    canvas.save(output, format='PNG')
    return output.getvalue()


def write_ico(frames, destination):
    offset = 6 + 16 * len(frames)
    entries = []
    for size, frame in frames.items():
        entries.append(struct.pack('<BBBBHHII', size % 256, size % 256, 0, 0, 1, 32, len(frame), offset))
        offset += len(frame)
    destination.write_bytes(struct.pack('<HHH', 0, 1, len(frames)) + b''.join(entries) + b''.join(frames.values()))


def main():
    assets = ROOT / 'assets'
    image = artwork(assets / 'app-icon.svg')
    frames = {size: png_frame(image, size) for size in ICO_SIZES}
    write_ico(frames, assets / 'app-icon.ico')
    for size in (128, 256):
        (assets / f'app-icon-{size}.png').write_bytes(frames[size])
    chunks = []
    for size, kind in ICNS_TYPES.items():
        frame = png_frame(image, size, macos=True)
        chunks.append(kind + struct.pack('>I', len(frame) + 8) + frame)
    chunks = b''.join(chunks)
    (assets / 'app-icon.icns').write_bytes(b'icns' + struct.pack('>I', len(chunks) + 8) + chunks)


if __name__ == '__main__':
    main()
