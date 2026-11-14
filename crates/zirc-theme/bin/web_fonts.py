"""Cuts the web fonts zirc-theme.css names from the whole ones in fonts/.

usage: web_fonts.py <zirc-theme.css> <out dir> <fonts/<name>.woff2>...

Each @font-face's file is `<name>-<part>.woff2`, holding the code points its unicode-range
lists (lib.rs's fonts::LATIN, and the rest), cut from `<name>.woff2`. The stylesheet decides
the files and their ranges; this only makes them.
"""

import re
import sys
from pathlib import Path

from fontTools import subset

FACE = re.compile(r'src: url\("fonts/([^"]+)-[a-z]+\.woff2"\)[^}]*?unicode-range: ([^;]+);')
FILE = re.compile(r'src: url\("fonts/([^"]+)"\)')


def main() -> None:
    css, out, *fonts = sys.argv[1:]
    sources = {Path(font).stem: Path(font) for font in fonts}
    stylesheet = Path(css).read_text()
    faces = FACE.findall(stylesheet)
    files = FILE.findall(stylesheet)
    if len(faces) != len(files):
        sys.exit(f"web_fonts.py: {len(files) - len(faces)} of {css}'s fonts have no unicode-range")
    covered: dict[str, set[int]] = {}
    for name, ranges in faces:
        covered.setdefault(name, set()).update(subset.parse_unicodes(ranges.replace(" ", "")))
    # Every character a whole font has is in one of its parts: one left out would be drawn in
    # another font, which nobody would notice until a page drew it.
    for name, points in covered.items():
        lost = set(subset.load_font(str(sources[name]), subset.Options()).getBestCmap()) - points
        if lost:
            sys.exit(f"web_fonts.py: {name}'s parts leave out {', '.join(f'U+{c:04X}' for c in sorted(lost))}")
    for file, (name, ranges) in zip(files, faces, strict=True):
        options = subset.Options()
        options.flavor = "woff2"
        # Everything the font has for those characters: its features, names and hinting.
        options.layout_features = ["*"]
        options.name_IDs = ["*"]
        options.name_languages = ["*"]
        options.notdef_outline = True
        options.hinting = True
        font = subset.load_font(str(sources[name]), options)
        subsetter = subset.Subsetter(options)
        subsetter.populate(unicodes=subset.parse_unicodes(ranges.replace(" ", "")))
        subsetter.subset(font)
        subset.save_font(font, str(Path(out) / file), options)


if __name__ == "__main__":
    main()
