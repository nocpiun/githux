# Third-Party Notices

Githux redistributes the third-party components listed here. This file is an
attribution index for packaging; the exact versions, hashes, distribution form
and the list of things still uncovered are in
[docs/third-party-assets.md](docs/third-party-assets.md).

This is **not legal advice**, and the presence of a file in this repository does
not mean a packaged binary already carries it. **Packaging is not complete.** See
"Required files at packaging time" below.

Baseline: `730d2292dcd5bb981928341259c971e42ff497b5`.

## Fonts compiled into the executable

| Font | License | Copyright | Full text |
| --- | --- | --- | --- |
| Noto Sans Regular / Medium / SemiBold (NotoSans-v2.015) | SIL OFL 1.1 | Copyright 2022 The Noto Project Authors | [assets/fonts/noto-sans/OFL.txt](assets/fonts/noto-sans/OFL.txt) |
| Google Sans Code (v7.001) | SIL OFL 1.1 | Copyright 2025 The Google Sans Code Project Authors | [assets/fonts/google-sans-code/OFL.txt](assets/fonts/google-sans-code/OFL.txt) |
| IBM Plex Sans Regular (via `ely-core`) | SIL OFL 1.1 | Copyright © 2017 IBM Corp. with Reserved Font Name "Plex" | [assets/licenses/ibm-plex-sans-ofl.txt](assets/licenses/ibm-plex-sans-ofl.txt) |

The adjacent `OFL.txt` files are **not** compiled into the binary; a packaged
distribution must ship them.

## Icons compiled into the executable

Lucide icons, bundled by `ely-core` and embedded via `include_bytes!`.

- License: ISC (Lucide Icons and Contributors).
- A subset derived from Feather: MIT, Copyright (c) 2013-present Cole Bemis (the
  list of derived names is inside the license file).
- Full text: [assets/licenses/lucide-icons-isc.txt](assets/licenses/lucide-icons-isc.txt).

## Copied source code

- `src/ui/reader/markdown/` is derived from Ely's `ely-documents` reader at
  revision `30c66fe1d5d23db769a6c834b771d054d7f288cf` (MIT). See
  [docs/content-reader-markdown-patch.md](docs/content-reader-markdown-patch.md).
- Ely (MIT) full text: [assets/licenses/ely-mit.txt](assets/licenses/ely-mit.txt).

## Compiled dependencies

| Component | Version | License | Copyright / full text |
| --- | --- | --- | --- |
| Ely (`ely-core`, `ely-settings`, `ely-canvas`) | git `30c66fe1` | MIT | Copyright (c) 2026 Ely GPUI Component contributors — [ely-mit.txt](assets/licenses/ely-mit.txt) |
| GPUI (`gpui-pre`, `gpui-pre-platform`, `gpui-pre-*`) | 0.3.10 | Apache-2.0 | Copyright 2022 - 2025 Zed Industries, Inc. — [gpui-pre-apache-2.0.txt](assets/licenses/gpui-pre-apache-2.0.txt) |
| pulldown-cmark | 0.13.5 | MIT | Copyright 2015 Google Inc. — [pulldown-cmark-mit.txt](assets/licenses/pulldown-cmark-mit.txt) |
| rusqlite | 0.37.0 | MIT | Copyright (c) 2014 The rusqlite developers (crate `LICENSE`) |
| SQLite (bundled by `rusqlite`/`libsqlite3-sys`) | 3.x amalgamation | Public Domain | [sqlite-public-domain.txt](assets/licenses/sqlite-public-domain.txt) |
| tokio | 1.54.1 | MIT | Copyright (c) Tokio Contributors (crate `LICENSE`) |
| serde, serde_json | 1.0.229 / 1.0.154 | MIT OR Apache-2.0 | crate `LICENSE-MIT` / `LICENSE-APACHE` |
| image | 0.25.10 | MIT OR Apache-2.0 | crate license files |
| reqwest | 0.13.5 | MIT OR Apache-2.0 | crate license files |
| base64 | 0.22.1 | MIT OR Apache-2.0 | Copyright (c) 2015 Alice Maz (MIT) |
| dirs | 7.0.0 | MIT OR Apache-2.0 | Copyright (c) 2018-2019 dirs-rs contributors (MIT) |
| thiserror | 2.0.21 | MIT OR Apache-2.0 | crate license files |
| unicode-segmentation | 1.13.3 | MIT OR Apache-2.0 | Copyright (c) 2015 The Rust Project Developers (MIT) |
| uuid | 1.28.0 | Apache-2.0 OR MIT | crate license files |
| raw-window-handle (Windows) | 0.6.2 | MIT OR Apache-2.0 OR Zlib | crate license files |
| windows, windows-core, windows-future (Windows) | 0.62.2 / 0.62.2 / 0.3.2 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation (MIT) |
| tempfile (dev only, not distributed) | 3.27.0 | MIT OR Apache-2.0 | crate license files |

The remaining transitive crates in `Cargo.lock` (748 packages in total) are
listed with their declared SPDX identifiers in
[docs/third-party-assets.md](docs/third-party-assets.md). Several are dual- or
multi-licensed; **no license choice has been made on the project's behalf**, and
build-only / dev-only / non-target-platform crates have not yet been separated
from the shipping set.

## Windows App SDK

The app-local Windows App SDK runtime is prepared by
[scripts/windows-runtime-lib.ps1](scripts/windows-runtime-lib.ps1) from two
pinned NuGet packages, and that script already copies each package's
`license.txt` next to the executable as `<package-id>.LICENSE.txt`:

- `microsoft.windowsappsdk.foundation 1.8.260803002`
- `microsoft.windowsappsdk.interactiveexperiences 1.8.260708001`

## Required files at packaging time

1. `assets/fonts/noto-sans/OFL.txt` and `assets/fonts/google-sans-code/OFL.txt`
   (not embedded in the binary).
2. `assets/licenses/ibm-plex-sans-ofl.txt`.
3. `assets/licenses/ely-mit.txt`.
4. `assets/licenses/lucide-icons-isc.txt`.
5. `assets/licenses/gpui-pre-apache-2.0.txt` and
   `assets/licenses/pulldown-cmark-mit.txt`, plus the license files of the
   remaining direct dependencies (and a resolved choice for dual-licensed
   crates).
6. `assets/licenses/sqlite-public-domain.txt` (informational).
7. Each Windows App SDK package's `license.txt` (copied by the runtime script).
8. This file.

No installer or release artifact exists yet; this task does not complete release
acceptance.
