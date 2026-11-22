"""Generate project-owned geometric test fonts. Requires FontTools to regenerate,
not to run the Rust tests. All outlines are synthetic, not copied from any font.
"""
from pathlib import Path
from fontTools.fontBuilder import FontBuilder
from fontTools.feaLib.builder import addOpenTypeFeaturesFromString
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTCollection

DIR = Path(__file__).parent


def make_font(family):
    cmap = {32: "space", 79: "O", 102: "f", 105: "i", 0x65E5: "jp_sun",
            0x672C: "jp_book", 0x8A9E: "jp_language"}
    names = [".notdef", *cmap.values(), "fi"]
    regions = {
        ".notdef": [[(0, 0), (0, 700), (500, 700), (500, 0)]],
        "O": [[(0, 0), (0, 700), (500, 700), (500, 0)],
              [(100, 100), (400, 100), (400, 600), (100, 600)]],
        "f": [[(0, 0), (0, 700), (300, 700), (50, 0)]],
        "i": [[(0, 0), (0, 450), (100, 450), (100, 0)],
              [(0, 550), (0, 700), (100, 700), (100, 550)]],
        "fi": [[(0, 0), (0, 700), (420, 700), (420, 0)]],
        "jp_sun": [[(0, 0), (200, 600), (400, 0)]],
        "jp_book": [[(0, 0), (200, 700), (400, 0)]],
        "jp_language": [[(0, 0), (200, 800), (400, 0)]],
    }
    glyphs = {}
    for name in names:
        pen = TTGlyphPen(None)
        for contour in regions.get(name, []):
            pen.moveTo(contour[0])
            for point in contour[1:]:
                pen.lineTo(point)
            pen.closePath()
        glyphs[name] = pen.glyph()
    builder = FontBuilder(1000, isTTF=True)
    builder.setupGlyphOrder(names)
    builder.setupCharacterMap(cmap)
    builder.setupGlyf(glyphs)
    builder.setupHorizontalMetrics({name: (600, 0) for name in names})
    builder.setupHorizontalHeader(ascent=850, descent=-150, lineGap=100)
    builder.setupNameTable({
        "familyName": family, "styleName": "Regular",
        "uniqueFontIdentifier": family + "-Regular",
        "psName": family.replace(" ", "") + "-Regular",
    })
    builder.setupOS2(sTypoAscender=850, sTypoDescender=-150, sTypoLineGap=100,
                     usWinAscent=850, usWinDescent=150)
    builder.setupPost()
    addOpenTypeFeaturesFromString(builder.font, "languagesystem latn dflt; feature liga { sub f i by fi; } liga;")
    builder.font.recalcTimestamp = False
    builder.font["head"].created = builder.font["head"].modified = 3849984000
    return builder.font


regular = make_font("Penplot Synthetic")
regular.save(DIR / "synthetic.ttf")
collection = TTCollection()
collection.fonts = [regular, make_font("Penplot Synthetic Alt")]
collection.save(DIR / "synthetic.ttc")
