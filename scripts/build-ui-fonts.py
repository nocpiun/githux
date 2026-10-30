"""Rebuild the private UI faces from fixed official Noto sources (fonttools==4.61.1).

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
import zipfile

from fontTools import subset
from fontTools.merge import Merger, Options
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

SC_REV = "6157dfaa640ca5738b4d1a7070fe389c3036d7fc"
SC_SHA = "a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da"
LATIN_SHA = "0c34df072a3fa7efbb7cbf34950e1f971a4447cffe365d3a359e2d4089b958f5"
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
    """Raw outline, hinting and advance; all selected sources use 1000 units/em."""
    name = font.getBestCmap()[cp]
    glyph = font["glyf"][name]
    coords, ends, flags = glyph.getCoordinates(font["glyf"])
    program = bytes(glyph.program.getBytecode()) if hasattr(glyph, "program") else b""
    return list(coords), list(ends), list(flags), font["hmtx"][name], program


def build(cache):
    cache.mkdir(parents=True, exist_ok=True)
    latin = source(
        cache, "NotoSans-v2.015.zip",
        "https://github.com/notofonts/latin-greek-cyrillic/releases/download/"
        "NotoSans-v2.015/NotoSans-v2.015.zip", LATIN_SHA,
    )
    sc = source(
        cache, "NotoSansSC.ttf",
        f"https://raw.githubusercontent.com/google/fonts/{SC_REV}/ofl/notosanssc/"
        "NotoSansSC%5Bwght%5D.ttf", SC_SHA,
    )
    out_dir = ROOT / "assets/fonts/githux-ui"
    out_dir.mkdir(parents=True, exist_ok=True)
    manifest = {}
    for weight, style in [(400, "Regular"), (500, "Medium"), (600, "SemiBold"), (700, "Bold")]:
        base = TTFont(io.BytesIO(zipfile.ZipFile(latin).read(
            f"NotoSans/hinted/ttf/NotoSans-{style}.ttf"
        )))
        chinese = instantiateVariableFont(TTFont(sc), {"wght": weight}, inplace=True)
        assert base["head"].unitsPerEm == chinese["head"].unitsPerEm == 1000
        # Disjoint cmaps: every original Noto Sans character wins. Only uncovered characters
        # (including all Han) are provided by Noto Sans SC; Latin never silently changes face.
        extra = chinese.getBestCmap().keys() - base.getBestCmap().keys()
        opts = subset.Options()
        opts.layout_features = ["*"]
        sub = subset.Subsetter(options=opts)
        sub.populate(unicodes=extra)
        sub.subset(chinese)
        # BASE's variable store / vertical layout / STAT are not applicable to this horizontal,
        # static UI family. Keep GSUB/GPOS, hinting, complete Han coverage and punctuation.
        options = Options()
        options.drop_tables = ["BASE", "STAT", "vhea", "vmtx"]
        base_path, sc_path = cache / f"latin-{weight}.ttf", cache / f"sc-{weight}.ttf"
        base.save(base_path)
        chinese.save(sc_path)
        merged = Merger(options=options).merge([str(base_path), str(sc_path)])
        # Stable private names avoid colliding with system Noto families or misrepresenting
        # this OFL derivative as an upstream release. Both sources retain their own notices.
        merged["name"].names = []
        names = {
            0: "Copyright 2022 The Noto Project Authors; Copyright 2014-2021 Adobe. "
               "Githux UI derivative, SIL Open Font License 1.1.",
            1: FAMILY, 2: style, 3: f"GithuxUI-1.000-{style}", 4: f"{FAMILY} {style}",
            5: "Version 1.000", 6: f"GithuxUI-{style}", 16: FAMILY, 17: style,
            13: "This Font Software is licensed under the SIL Open Font License, Version 1.1.",
            14: "https://openfontlicense.org/",
        }
        for key, value in names.items():
            merged["name"].setName(value, key, 3, 1, 0x409)
        merged["head"].created = merged["head"].modified = 3786912000
        merged.recalcTimestamp = False
        path = out_dir / f"GithuxUI-{style}.ttf"
        merged.save(path)
        actual = TTFont(path)
        base = TTFont(base_path)
        chinese = TTFont(sc_path)
        # Check *every* mapped glyph, not only a family name or the visual acceptance sample.
        for cp in base.getBestCmap():
            assert signature(actual, cp) == signature(base, cp), f"Latin changed: {cp:x}"
        for cp in extra:
            assert signature(actual, cp) == signature(chinese, cp), f"SC changed: {cp:x}"
        assert all(ord(c) in actual.getBestCmap() for c in SAMPLES)
        manifest[str(weight)] = {str(ord(c)): actual.getGlyphID(actual.getBestCmap()[ord(c)])
                                 for c in SAMPLES}
        print(f"{path.name}: {path.stat().st_size} B "
              f"sha256={hashlib.sha256(path.read_bytes()).hexdigest()} "
              f"latin={len(base.getBestCmap())} sc={len(extra)} weight={weight}")
    (out_dir / "sample-glyphs.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, required=True)
    build(parser.parse_args().cache)
