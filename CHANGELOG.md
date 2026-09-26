# Changelog

All notable changes to QM Unlock are documented here.

## [Unreleased]

### Added

- Design C "Darkroom" interface: dark/light themes with persisted preference and
  system default, slim overlay titlebar on macOS, pixel-art brand mark.
- Plain-audio channel: `.flac` / `.mp3` / `.m4a` / `.ogg` / `.opus` / `.wav` can be
  enhanced (cover / lyrics / library link) without decryption; mixed imports are
  split into 加密 / 普通 tabs that operate independently.
- Write mode for plain audio: in-place or copy, so original files are never
  modified silently.
- Independent LRC output directory, revealed only while the lyrics toggle is on.
- `+` button in the queue header to add files; `⌘O` / `⌘↩` shortcuts are now real.
- `os_platform` command so macOS-only library features are hidden on other
  platforms regardless of credential state.
- Styled DMG packaging: `desktop/scripts/make_dmg_styled.sh` produces a
  compressed read-only image with a blank 660x400 Finder window, 84px icons, the
  app on the left, Applications on the right, the install notes in the top-right
  corner and a custom volume icon. It runs headless, with no AppleScript and no
  third-party packaging tool.
- `desktop/scripts/dmg-template.DS_Store`, the committed layout template, and
  `dmg_layout.py`, which derives it from a Finder-approved sample by rewriting
  only the volume name (5 places in the Alias v2 record) and the item names,
  leaving window bounds, icon size and coordinates byte-identical.

### Changed

- CI packaging (`sign-and-package-macos.sh`) is now a thin wrapper that resolves
  the target, version and output path, then delegates to `make_dmg_styled.sh`, so
  releases and local builds share one signing, verification and layout path
  instead of CI emitting a plain unstyled DMG.
- Nested code (frameworks, helpers, dylibs, executables) is verified
  individually during packaging; `--deep` alone only covers the main executable.
- Progress is batch-relative (finished files plus the current file's phase
  fraction), so it advances smoothly instead of sitting at 0% until completion.
- The cover toggle writes the FLAC picture block plus the Finder custom icon;
  the embedded cover alone is invisible in Finder, so the pair must share one
  switch. QQ extended attributes and library linking require the 曲库 toggle.
- Completion and error feedback is a single auto-dismissing toast (6s); per-file
  detail moved to row tooltips.
- DMG is now a blank window: no background image at all (`backgroundType` 0, no
  alias, no bookmark). Light and dark system appearances both keep labels
  readable, and it removes the whole class of background rendering problems.
  The install notes sit in the top-right corner; app and Applications keep their
  centered row. Global icon size dropped from 96 to 84 (Finder has no per-item
  size, so the change is global).
- Titlebar decluttered: the `⌘O` / `⌘↩` hint chips and the manual credential
  refresh button are gone (window focus now refreshes credentials and library
  status automatically). The app name is written next to the pixel mark, because
  the overlay titlebar hides the system title.

### Fixed

- DMG window opened larger than 660x400: Finder restores the last window size per
  volume name and then ignores `WindowBounds`, so the background stretched and the
  icon cards looked skewed. A fresh volume name defeats the saved state.

- Per-row progress bars showed the batch percentage, so the first row finished
  almost instantly and the last row only filled at the very end. The backend now
  emits `filePercent` (the current file's own 0-100) alongside `percent`
  (batch-relative); rows use the former, the top line and footer use the latter.

- DMG opened with a white background: `dmgbuild` writes only the legacy
  `backgroundImageAlias` and omits the `pBBk` bookmark that modern Finder
  resolves first, and its alias was a truncated 324 bytes rather than 600+.
- `codesign --verify --strict` failed on the app inside the image because
  `makehybrid -hfs` attaches HFS file type/creator data that surfaces as 20
  `com.apple.FinderInfo` xattrs; the app is now cleaned inside the mounted
  volume, while the volume root and `.VolumeIcon.icns` keep theirs because the
  custom volume icon depends on them.
- Tauri's own bundle failed strict verification (`code has no resources but
  signature indicates they must be present`); packaging now clears xattrs and
  re-signs ad-hoc before building the image.
- Queue list could grow past the window, overlap the footer and refuse to scroll.
- Duplicated app name rows in the macOS titlebar (system title plus custom bar).
- Drag veil flickered when the pointer moved over child elements.
- Copy mode could modify the source or overwrite an existing file when the target
  equalled the source path or a same-name file; copy targets now take a
  「副本 / 副本 2 …」 suffix until they collide with neither.
- Library linking deleted stale rows by song name plus singer prefix, which could
  remove other versions of the same song; the DELETE predicate now matches only
  rows whose path equals the target or the same-directory encrypted source
  (`.mgg` / `.mmp4`).
- Cover embedding removed every PICTURE block, losing back covers and artist
  photos; only the front cover (type 3) is replaced, and verification now also
  asserts the other-picture count is unchanged.
- Same-second library backups overwrote each other; backup directories now take a
  `-1`, `-2` … suffix when the timestamped name is already taken.
- The packaging `find` expression applied `-print0` only to the file branch, so
  nested `.app` / `.framework` bundles skipped per-item signature verification;
  both branches are grouped before `-print0`.
- Plain-audio identity now prefers the file's own tags (FLAC Vorbis comments,
  MP3 ID3v2) for the search query and for a match check; when the search result
  disagrees with those tags the file is skipped instead of receiving another
  song's cover, lyric or library link.
- Cross-toolchain CI failures: UTF-16 tag decoding used `chunks_exact(2)`, which
  clippy 1.98 rejects via `chunks_exact_to_as_chunks`; it now uses
  `as_chunks::<2>()`. The FLAC STREAMINFO helpers are compiled only on macOS
  (plus tests), their sole production caller, so Windows builds no longer trip
  `dead_code` under `-D warnings`.
- ID3v2 parsing stopped at the first non-text frame (APIC, COMM, ...), losing
  title/artist frames placed after it; unrelated frames are now skipped by
  their declared size and parsing continues.
- ID3v2 tags are read only for the header-declared tag region instead of the
  whole audio file, avoiding full-file memory loads on large batches.
- Library search now returns every candidate and the caller walks them in
  order, picking the first that matches the embedded tags; validation also
  compares album, so same-title same-artist album variants no longer mismatch.

## [1.0.0] - 2026-08-25

### Added

- `musicex V1` support for `.mmp4` input and M4A output detection.
- Separate macOS Apple Silicon and Intel release packages.
- Compatibility matrix and detailed decryption technical documentation.

### Fixed

- Match the historical QMC2 `mapL` behavior required by current `musicex` files.

## [0.1.0] - 2026-08-25

### Added

- Cross-platform QM Unlock desktop application for macOS and Windows.
- Native file/folder drag-and-drop, queue progress, manual ekey fallback and MP3 conversion.
- CI verification and tag-triggered unsigned release builds.
