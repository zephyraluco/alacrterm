"""Regenerate committed PNG/ICNS icons after changing assets/app-icon.ico."""
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parents[2]


def icon_images(source):
    """Reuse the ICO's original PNG frames; no image-conversion dependency is needed."""
    data = source.read_bytes()
    reserved, kind, count = struct.unpack_from('<HHH', data)
    if reserved != 0 or kind != 1:
        raise ValueError('Expected an ICO application icon')
    images = {}
    for index in range(count):
        width, height, _, _, _, _, length, offset = struct.unpack_from('<BBBBHHII', data, 6 + index * 16)
        width, height = width or 256, height or 256
        frame = data[offset:offset + length]
        if width == height and frame.startswith(b'\x89PNG\r\n\x1a\n'):
            images[width] = frame
    if not {128, 256}.issubset(images):
        raise ValueError('Application ICO must contain 128px and 256px PNG images')
    return images


def write_icns(source, destination):
    images = icon_images(source)
    chunks = b''.join(kind + struct.pack('>I', len(images[size]) + 8) + images[size]
                      for size, kind in [(128, b'ic07'), (256, b'ic08')])
    destination.write_bytes(b'icns' + struct.pack('>I', len(chunks) + 8) + chunks)


def main():
    assets = ROOT / 'assets'
    source = assets / 'app-icon.ico'
    for size, image in icon_images(source).items():
        (assets / f'app-icon-{size}.png').write_bytes(image)
    write_icns(source, assets / 'app-icon.icns')


if __name__ == '__main__':
    main()
