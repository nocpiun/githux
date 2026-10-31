# Bundled fonts

The application embeds a private UI family, **Githux UI**, and **Google Sans Code** in [`fonts.rs`](../../src/platform/fonts.rs). Githux UI uses the original **Noto Sans** outlines for every character Noto Sans covers (Latin/Greek/Cyrillic, 2,965 mappings), plus **Noto Sans SC** outlines for the remaining 30,361 mappings, including simplified Chinese and CJK punctuation. It has real 400/500/600/700 upright faces. The private name identifies an OFL derivative, avoids collisions with installed Noto fonts, and does not change the approved Latin/Chinese glyph sources.

GPUI 0.3.10 on Windows builds `FontFallbacks` from the **system** font collection (`direct_write.rs::generate_font_fallbacks`). Adding a font to GPUI's custom collection and naming it as a fallback therefore cannot guarantee app-local Chinese. The composite face provides Chinese directly in the registered in-memory face; no installed Noto Sans SC or OS font setting is required. Latin is not replaced with SC Latin. Google Sans Code remains unchanged and separate for source, inline code and fenced blocks; unsupported code characters and emoji may still use the platform fallback.

| Embedded face | Bytes | SHA-256 |
| --- | ---: | --- |
| `githux-ui/GithuxUI-Regular.ttf` | 10930732 | `66d4f46692de47e13d9762380543cec42b30666752a682abd0fbb8c253f64f7f` |
| `githux-ui/GithuxUI-Medium.ttf` | 10922280 | `09789b932110ae403a37abd0c37613ac9dc6d91b2ce561d77d7b10d314e4c763` |
| `githux-ui/GithuxUI-SemiBold.ttf` | 10923796 | `e7a5aa03d4be8acd553df198db3c14eb8f2b0e38711134cfe1078f1d51d1e0f3` |
| `githux-ui/GithuxUI-Bold.ttf` | 10930412 | `cfd858e4667e76f29b792f629df6dc45b0b6ea1ed06b92263b9a114248de9033` |
| `google-sans-code/GoogleSansCode.ttf` | 268264 | `74642762fae66d5883077794a763e1086394b8cb72f2623e456e06359dc5c64b` |

UI faces total 43,707,220 B (41.68 MiB), replacing 1,866,600 B of Latin-only faces. This supplies complete SC source coverage at four actual weights instead of shipping a sample-only subset or relying on synthesis. No other Noto language collection is included.

Rebuild with Python and `fonttools==4.61.1`:

```sh
python scripts/build-ui-fonts.py --cache .agents/task/font-sources
```

The script downloads checksum-verified official sources, instantiates SC at 400/500/600/700, removes SC cmap entries already covered by Noto Sans, merges horizontal shaping tables and hinting, assigns private family names, and fixes timestamps. It removes only nonapplicable vertical/variable BASE/STAT tables; GSUB/GPOS and horizontal glyph coverage remain. It verifies the outline points, contour endpoints, flags and advances of **every** source mapping after serialization. `sample-glyphs.json` binds the native `examples/fonts.rs` acceptance to the embedded cmap. Python is build-time resource tooling only; Cargo uses the committed TTFs.

Official sources:

- [Noto Sans v2.015](https://github.com/notofonts/latin-greek-cyrillic/releases/tag/NotoSans-v2.015): `NotoSans/hinted/ttf/NotoSans-{Regular,Medium,SemiBold,Bold}.ttf` from [release archive](https://github.com/notofonts/latin-greek-cyrillic/releases/download/NotoSans-v2.015/NotoSans-v2.015.zip), SHA-256 `0c34df072a3fa7efbb7cbf34950e1f971a4447cffe365d3a359e2d4089b958f5`.
- [Noto Sans SC, Google Fonts revision 6157dfaa](https://github.com/google/fonts/tree/6157dfaa640ca5738b4d1a7070fe389c3036d7fc/ofl/notosanssc): `NotoSansSC[wght].ttf`, version `2.004-H2`, 17,772,300 B, SHA-256 `a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da`.
- [Google Sans Code v7.001](https://github.com/googlefonts/googlesans-code/releases/tag/v7.001): `GoogleSansCode[MONO,wght].ttf`; archive SHA-256 `eebba2a438882396e8530cad6d6cb80182b0afb6e9e0ad07b7340359a53ba2e9`.

All use SIL OFL 1.1. Ship `githux-ui/OFL-NotoSans.txt`, `githux-ui/OFL-NotoSansSC.txt` and `google-sans-code/OFL.txt` with distributions. Source names/copyrights are retained in those notices; the derivative uses a new primary family name. Ely also embeds mandatory IBM Plex Sans and Lucide icons; optional Inter/JetBrains Mono are disabled. See [third-party assets](../../docs/third-party-assets.md) and [notices](../../THIRD_PARTY_NOTICES.md). Windows native sample/DPI boundaries are recorded in [the font devlog](../../docs/devlog/2026-10-29-ui-fonts.md).
