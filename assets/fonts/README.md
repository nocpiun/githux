# Bundled fonts

The application embeds a private UI family, **Githux UI**, and **Google Sans Code** in [`fonts.rs`](../../src/platform/fonts.rs). Githux UI is generated from **Noto Sans SC** (simplified Chinese, Latin, digits, CJK punctuation, 30,890 mappings). All UI glyphs (Latin, digits, punctuation, and Han characters) come from Noto Sans SC directly at real 400/500/600/700 upright faces. The private name identifies an OFL derivative, avoids collisions with installed Noto fonts, and provides both Latin and CJK from the same font source.

GPUI 0.3.10 on Windows builds `FontFallbacks` from the **system** font collection (`direct_write.rs::generate_font_fallbacks`). Adding a font to GPUI's custom collection and naming it as a fallback therefore cannot guarantee app-local Chinese. Embedding Noto Sans SC directly as the primary UI family provides complete Latin and CJK character support in the registered in-memory face; no installed Noto Sans SC or OS font setting is required. Google Sans Code remains unchanged and separate for source, inline code and fenced blocks; unsupported code characters and emoji may still use the platform fallback.

| Embedded face | Bytes | SHA-256 |
| --- | ---: | --- |
| `githux-ui/GithuxUI-Regular.ttf` | 10470908 | `0a9b1f37d0d8c33d62b973f499069777ec3022f01f16f9878a3ffd8396dc88df` |
| `githux-ui/GithuxUI-Medium.ttf` | 10464048 | `0ff053e20d270189e99177e0514ef77ba208a509acbf14dd46e8c336c7525f89` |
| `githux-ui/GithuxUI-SemiBold.ttf` | 10460532 | `7bf6a0c5e0fd230b92e8c32ecaba54ef27d2561684d342072058f02aef31bbee` |
| `githux-ui/GithuxUI-Bold.ttf` | 10460408 | `2f2ecd239eee7c3f0bf3a72d5ba36af3b9a9a95665d39d4f30a33f5bcf7c773d` |
| `google-sans-code/GoogleSansCode.ttf` | 268264 | `74642762fae66d5883077794a763e1086394b8cb72f2623e456e06359dc5c64b` |

UI faces total 41,855,896 B (39.92 MiB). This supplies complete Noto Sans SC source coverage at four actual weights without relying on synthetic bolding or system fallback.

Rebuild with Python and `fonttools==4.61.1`:

```sh
python scripts/build-ui-fonts.py --cache .agents/task/font-sources
```

The script downloads checksum-verified official source `NotoSansSC[wght].ttf`, instantiates SC at 400/500/600/700, assigns private family names, and fixes timestamps. It removes only nonapplicable vertical/variable BASE/STAT tables; GSUB/GPOS, hinting, and horizontal glyph coverage remain intact. It verifies the outline points, contour endpoints, flags and advances of **every** mapping against the instantiated source. `sample-glyphs.json` binds the native `examples/fonts.rs` acceptance to the embedded cmap. Python is build-time resource tooling only; Cargo uses the committed TTFs.

Official sources:

- [Noto Sans SC, Google Fonts revision 6157dfaa](https://github.com/google/fonts/tree/6157dfaa640ca5738b4d1a7070fe389c3036d7fc/ofl/notosanssc): `NotoSansSC[wght].ttf`, version `2.004-H2`, 17,772,300 B, SHA-256 `a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da`.
- [Google Sans Code v7.001](https://github.com/googlefonts/googlesans-code/releases/tag/v7.001): `GoogleSansCode[MONO,wght].ttf`; archive SHA-256 `eebba2a438882396e8530cad6d6cb80182b0afb6e9e0ad07b7340359a53ba2e9`.

All use SIL OFL 1.1. Ship `githux-ui/OFL-NotoSansSC.txt` and `google-sans-code/OFL.txt` with distributions. Source names/copyrights are retained in those notices; the derivative uses a new primary family name. Ely also embeds mandatory IBM Plex Sans and Lucide icons; optional Inter/JetBrains Mono are disabled. See [third-party assets](../../docs/third-party-assets.md) and [notices](../../THIRD_PARTY_NOTICES.md). Windows native sample/DPI boundaries are recorded in [the font devlog](../../docs/devlog/2026-10-29-ui-fonts.md).
