# Rune patches

`vendor/rune` is anomalyco/rune at `ae0c94990cf7c8c0037de36d912ccb9c794db8a9`
(the rev Hex's `Cargo.toml` pins), trimmed to the crates Hex builds, with
these changes. Hex uses it through `[patch."https://github.com/anomalyco/rune.git"]`.

When bumping Rune, copy the same crates from the new rev, reapply each patch
below, and update the rev here and in `Cargo.toml`.

## A window that releases its GPU on close gets its own device

`crates/rune/src/window.rs`, where the window builds its GPU: with
`App::release_gpu_when_closed`, the window neither takes the device the
overlays share from `gpu_pool` nor publishes its own there. On the shared
device, the Metal command-buffer pools its frames grew stayed with the
overlays after the window closed (Hex kept ~130 MB after a settings visit).
