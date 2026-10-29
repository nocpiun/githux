# Bundled fonts

`NotoSans-Regular.ttf`, `NotoSans-Medium.ttf`, `NotoSans-SemiBold.ttf` and `GoogleSansCode.ttf` are compiled into the executable with `include_bytes!` from [`src/platform/fonts.rs`](../../src/platform/fonts.rs) and registered with GPUI at startup. The application never reads fonts from the working directory or an absolute system path. Both families use the SIL Open Font License 1.1; each font has its license text beside it (`noto-sans/OFL.txt`, `google-sans-code/OFL.txt`).

The adjacent `OFL.txt` files are kept in this repository only; they are not compiled into the binary, so a packaged distribution must ship them separately. See [third-party assets](../../docs/third-party-assets.md) and [THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md). The application also links `ely-core`, which always embeds its own IBM Plex Sans (OFL, for GPUI's SVG renderer) and its Lucide icon set; this build disables Ely's optional Inter / JetBrains Mono fonts (`default-features = false`), so those two families are not distributed.

| Family | Fixed official release | Selected file | SHA-256 (packaged file) |
| --- | --- | --- | --- |
| Noto Sans Regular | [NotoSans-v2.015](https://github.com/notofonts/latin-greek-cyrillic/releases/tag/NotoSans-v2.015) | `NotoSans/hinted/ttf/NotoSans-Regular.ttf` | `478c558ea716033cd60c03438f628dfa75694dcf6b5f6d505a2f05fd2b4f3823` |
| Noto Sans Medium | NotoSans-v2.015 | `NotoSans/hinted/ttf/NotoSans-Medium.ttf` | `635d93d1131d791f2576de90b3bb0f7cdf61929906e8420a61b5f7f8e76420bb` |
| Noto Sans SemiBold | NotoSans-v2.015 | `NotoSans/hinted/ttf/NotoSans-SemiBold.ttf` | `a4e91fd530ac2b4ef5367240144ff37d7d65d66cf76f2e9a2187b93c676f92d0` |
| Google Sans Code | [v7.001](https://github.com/googlefonts/googlesans-code/releases/tag/v7.001) | `GoogleSansCode[MONO,wght].ttf`, stored as `GoogleSansCode.ttf` | `74642762fae66d5883077794a763e1086394b8cb72f2623e456e06359dc5c64b` |

Official downloads:

- <https://github.com/notofonts/latin-greek-cyrillic/releases/download/NotoSans-v2.015/NotoSans-v2.015.zip> — archive SHA-256 `0c34df072a3fa7efbb7cbf34950e1f971a4447cffe365d3a359e2d4089b958f5`.
- <https://github.com/googlefonts/googlesans-code/releases/download/v7.001/GoogleSansCode-v7.001.zip> — archive SHA-256 `eebba2a438882396e8530cad6d6cb80182b0afb6e9e0ad07b7340359a53ba2e9`.

Noto Sans ships three static upright faces from one release because GPUI selects a face by requested weight: `MEDIUM`/`SEMIBOLD` requests (active rows, labels, headings, badges) need the real Medium and SemiBold files. With only Regular registered, DirectWrite answered those requests with a substituted/synthetic face, which is why the UI could look as though Noto Sans had not been applied while 400-weight text was in fact correct. `Bold` (700) is still not bundled and is not used by the current shell; if a design needs it, add the same release's Bold file rather than relying on synthesis. Google Sans Code stays one upright variable file, and its `wght` axis covers the weights code uses.

Noto Sans covers the Latin/Greek/Cyrillic family here; this is not an all-language font collection. Noto Sans is the UI font (the theme `font_family` set in [`src/app/mod.rs`](../../src/app/mod.rs)); Google Sans Code is the code font (the theme `mono_family`), used by the source viewer, Markdown fenced blocks and inline code. CJK and emoji fall back to installed system fonts (on the verified Windows machine, `Microsoft YaHei UI` for the sampled Chinese strings); CJK weights therefore follow the fallback face, not Noto Sans. Complex shaping, variable axes and additional weights beyond the three bundled faces require separate visual/input acceptance.
