# Bundled fonts

These files are embedded with `include_bytes!` and registered with GPUI during startup. The application does not read fonts from the current directory or any absolute system path. Both families use the SIL Open Font License 1.1; retain each adjacent `OFL.txt` when distributing the binary. Ely additionally bundles its own upstream assets and fonts.

| Family | Fixed official release | Selected file | SHA-256 (packaged file) |
| --- | --- | --- | --- |
| Noto Sans | [NotoSans-v2.015](https://github.com/notofonts/latin-greek-cyrillic/releases/tag/NotoSans-v2.015) | `NotoSans/hinted/ttf/NotoSans-Regular.ttf` | `478c558ea716033cd60c03438f628dfa75694dcf6b5f6d505a2f05fd2b4f3823` |
| Google Sans Code | [v7.001](https://github.com/googlefonts/googlesans-code/releases/tag/v7.001) | `GoogleSansCode[MONO,wght].ttf`, stored as `GoogleSansCode.ttf` | `74642762fae66d5883077794a763e1086394b8cb72f2623e456e06359dc5c64b` |

Official downloads:

- <https://github.com/notofonts/latin-greek-cyrillic/releases/download/NotoSans-v2.015/NotoSans-v2.015.zip> — archive SHA-256 `0c34df072a3fa7efbb7cbf34950e1f971a4447cffe365d3a359e2d4089b958f5`.
- <https://github.com/googlefonts/googlesans-code/releases/download/v7.001/GoogleSansCode-v7.001.zip> — archive SHA-256 `eebba2a438882396e8530cad6d6cb80182b0afb6e9e0ad07b7340359a53ba2e9`.

Only one upright file per family is included. Noto Sans covers the Latin/Greek/Cyrillic family here; this is not an all-language font collection. The Hello heading/button use Noto Sans, and the visible counter uses Google Sans Code. CJK, emoji fallback, complex shaping, variable axes and additional weights require separate visual/input acceptance.
