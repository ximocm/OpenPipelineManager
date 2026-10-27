# Third-Party Notices

This project depends on third-party open-source packages. Keep each upstream
license and copyright notice when redistributing source, packaged builds,
containers, or bundles. This checklist covers the direct dependencies declared
in the `core/*/Cargo.toml` and `ui/Cargo.toml` manifests; release artifacts
additionally carry a transitive-dependency inventory in
`third-party/inventory.json`, generated from `cargo metadata` by the packaging
scripts in `scripts/`.

This is a practical compliance checklist, not legal advice.

## Provenance

Portions of the core and UI are cloned and adapted from
[minmark](https://github.com/ximocm/minmark), © ximocm, licensed under
Apache-2.0. See `NOTICE` and `third-party/minmark-Apache-2.0.txt`; every
packaged build stages both files.

## Core

| Package | License | Attribution to retain | Source |
| --- | --- | --- | --- |
| pulldown-cmark | MIT | Copyright (c) 2015 Raph Levien and the pulldown-cmark authors | https://github.com/raphlinus/pulldown-cmark |
| serde | MIT OR Apache-2.0 | Copyright (c) 2014 The serde-rs developers | https://github.com/serde-rs/serde |
| serde_json | MIT OR Apache-2.0 | Copyright (c) 2017 the serde-rs/json developers | https://github.com/serde-rs/json |
| serde_yml | MIT OR Apache-2.0 | Copyright (c) 2023-2024 Sébastien Rousseau (serde_yaml fork) | https://github.com/sebastienrousseau/serde_yml |
| chrono | MIT OR Apache-2.0 | Copyright (c) 2014–2025 the chrono developers | https://github.com/chronotope/chrono |
| libc | MIT OR Apache-2.0 | Copyright (c) 2014–2025 the Rust Project Developers | https://github.com/rust-lang/libc |

## UI

| Package | License | Attribution to retain | Source |
| --- | --- | --- | --- |
| gtk4 | MIT | Copyright (c) the gtk-rs team | https://github.com/gtk-rs/gtk4-rs |
| glib / gio / gdk4 / pango / cairo-rs / graphene-rs | MIT | Copyright (c) the gtk-rs team | https://github.com/gtk-rs/gtk-rs-core |
| directories | MIT OR Apache-2.0 | Copyright (c) 2018 dirs-rs contributors | https://github.com/soc/directories-rs |

## Notable transitive dependencies

Transitive crates are covered by `third-party/inventory.json` in release
artifacts. Call out `option-ext` (MPL-2.0, pulled in by `directories`/`dirs-sys`):
MPL-2.0 is a file-level copyleft license; using the crate as a library
dependency is compatible with this project, but modified MPL-2.0 files must
keep their license notices and remain available under MPL-2.0.
