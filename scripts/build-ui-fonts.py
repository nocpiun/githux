"""Rebuild the private UI faces directly from official Noto Sans SC (fonttools==4.61.1).

Run: python scripts/build-ui-fonts.py --cache .agents/task/font-sources
Only development tooling uses Python/fontTools; Cargo embeds the resulting TTFs.
No installed font or operating-system font setting is read or modified.
"""

import argparse
import hashlib
import io
import json
from pathlib import Path
import urllib.request

from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

SC_REV = "6157dfaa640ca5738b4d1a7070fe389c3036d7fc"
SC_SHA = "a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da"
FAMILY = "Githux UI"
SAMPLES = "Minecraft 服务器管理面板简介功能特性菜单栏金额额度显示中文"
ROOT = Path(__file__).resolve().parent.parent


def source(cache, name, url, expected):
    path = cache / name
    if not path.exists():
        with urllib.request.urlopen(url) as response:
            path.write_bytes(response.read())
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != expected:
        raise ValueError(f"{name}: SHA-256 mismatch: {actual}")
    return path


def signature(font, cp):
    """Raw outline, hinting and advance; source uses 1000 units/em."""
    name = font.getBestCmap()[cp]
    glyph = font["glyf"][name]
    coords, ends, flags = glyph.getCoordinates(font["glyf"])
    program = bytes(glyph.program.getBytecode()) if hasattr(glyph, "program") else b""
    return list(coords), list(ends), list(flags), font["hmtx"][name], program


def build(cache):
    cache.mkdir(parents=True, exist_ok=True)
    sc = source(
        cache, "NotoSansSC.ttf",
        f"https://raw.githubusercontent.com/google/fonts/{SC_REV}/ofl/notosanssc/"
        "NotoSansSC%5Bwght%5D.ttf", SC_SHA,
    )
    out_dir = ROOT / "assets/fonts/githux-ui"
    out_dir.mkdir(parents=True, exist_ok=True)
    manifest = {}
    for weight, style in [(400, "Regular"), (500, "Medium"), (600, "SemiBold"), (700, "Bold")]:
        font = instantiateVariableFont(TTFont(sc), {"wght": weight}, inplace=True)
        assert font["head"].unitsPerEm == 1000
        # BASE's variable store / vertical layout / STAT are not applicable to this horizontal,
        # static UI family. Keep GSUB/GPOS, hinting, complete Han & Latin coverage and punctuation.
        for table in ["BASE", "STAT", "vhea", "vmtx"]:
            if table in font:
                del font[table]
        # Stable private names avoid colliding with system Noto families or misrepresenting
        # this OFL derivative as an upstream release.
        font["name"].names = []
        names = {
            0: "Copyright 2014-2021 Adobe (http://www.adobe.com/), with Reserved Font Name 'Source'. "
               "Githux UI derivative, SIL Open Font License 1.1.",
            1: FAMILY, 2: style, 3: f"GithuxUI-1.000-{style}", 4: f"{FAMILY} {style}",
            5: "Version 1.000", 6: f"GithuxUI-{style}", 16: FAMILY, 17: style,
            13: "This Font Software is licensed under the SIL Open Font License, Version 1.1.",
            14: "https://openfontlicense.org/",
        }
        for key, value in names.items():
            font["name"].setName(value, key, 3, 1, 0x409)
        font["head"].created = font["head"].modified = 3786912000
        font.recalcTimestamp = False

        path = out_dir / f"GithuxUI-{style}.ttf"
        font.save(path)

        # Verification: compare every glyph against the re-instantiated source face.
        actual = TTFont(path)
        raw_source = instantiateVariableFont(TTFont(sc), {"wght": weight}, inplace=True)
        buf = io.BytesIO()
        raw_source.save(buf)
        buf.seek(0)
        reloaded_source = TTFont(buf)

        source_cmap = reloaded_source.getBestCmap()
        actual_cmap = actual.getBestCmap()
        assert source_cmap == actual_cmap, f"Cmap mismatch for {style}"

        for cp in actual_cmap:
            assert signature(actual, cp) == signature(reloaded_source, cp), f"Glyph mismatch at {cp:x}"

        assert all(ord(c) in actual_cmap for c in SAMPLES)
        manifest[str(weight)] = {str(ord(c)): actual.getGlyphID(actual_cmap[ord(c)])
                                 for c in SAMPLES}
        print(f"{path.name}: {path.stat().st_size} B "
              f"sha256={hashlib.sha256(path.read_bytes()).hexdigest()} "
              f"glyphs={len(actual_cmap)} weight={weight}")

    (out_dir / "sample-glyphs.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, required=True)
    build(parser.parse_args().cache)
