# VTE Fork Notes

Forked from alacritty/vte v0.15.0 (crates.io release).

## Changes from upstream

Two insertions in `src/ansi.rs`:

1. **Handler trait** (~line 733): Added `set_extra_cursors(&mut self, _params: &[&[u16]])` method with default no-op body. This exposes the kitty multiple cursors protocol to terminal implementations.

2. **CSI dispatch** (~line 1721): Added match arm `('q', [b'>', b' '])` to dispatch `CSI > Ps ; ... SP q` sequences to `handler.set_extra_cursors()`.

No other files were modified. `src/lib.rs` and `src/params.rs` are byte-identical to upstream.

## Known limitations

VTE's parser supports a maximum of 32 parameter groups (MAX_PARAMS). Since the
kitty multi-cursor protocol uses one param group per cursor (plus one for shape),
this limits separate-param-group encoding to ~31 cursors. Clients can work around
this by packing multiple y:x pairs into a single param group (e.g.,
`2:y1:x1:y2:x2`), which the parser handles correctly up to the 64-cursor limit.

## Rebase guide

When updating to a new upstream VTE release, only `src/ansi.rs` needs manual reconciliation. Search for `set_extra_cursors` to find both insertion points.
