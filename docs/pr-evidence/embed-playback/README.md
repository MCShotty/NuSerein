# Embedded video playback

The matched pair uses fork main `5775101829dde47d3319228ed711f3c5ff485d5d`
and this PR's implementation. Both are native synthetic previews at 1440 × 1100,
display scale 1, dark appearance, with the same fonts and fixture:

```sh
cargo build --locked -p serein --features demo --example profile_preview
profile_preview --demo --page=embeds --media-embeds --provider-embed --interactive --width=1440 --height=1100
```

After two seconds, click the YouTube poster and wait another 0.8 seconds. Before,
Play replaces the conversation with a modal. After, the player rectangle remains
inside its message card. Enlarge/fullscreen require separate actions. The light
layout was also inspected at 760 × 1100; narrow card controls wrap.

These previews never create an external provider view or access an owner session.
They establish host layout, not successful YouTube playback. The shipping wrapper
has separate offline API tests, and decoder regressions use generated media and a
synthetic audio sink. Windows/macOS clipping, focus, fullscreen and provider
interoperability still require native acceptance. The Linux native WebKit fixture
cannot find its helper executable in this workspace's staged SDK configuration;
no browser sandbox was disabled.

Current release measurements and their methods are recorded in
[performance.md](../../performance.md#embedded-playback-recovery-october-21-2026).
