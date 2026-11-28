# Memo: the laser design page looks like a tile, not a page

Internal, 2026-11-28. Not an issue (owner: "zet het in een interne memo. geen issue").

## What the owner sees

`/laser-design/` on the hub looks like a card on a page instead of a page of Maker Connect.

## Why

`crates/hub/src/laser_design.rs` (`fn index`) puts LaserFoundry in an `<iframe class="ld-frame">` that has:

- the house's tile shape: `clip-path` with the 45° cut top right (`--cut-l`);
- a fixed height of `calc(100dvh - 190px)` (`- 250px` under 760 px), not the room that is left;
- a page head of its own above it (title, a line of explanation, a "Full screen" button).

LaserFoundry's own colours and fonts inside the frame are its own, not the hub's.

## What we could do, smallest first

1. **CSS only, in `laser_design.rs` (about an hour).** No tile shape on the frame; the head one thin line; the frame fills
   the space beside the sidebar to the bottom of the window. "Full screen" stays.
2. **The house style inside LaserFoundry.** In our fork (`CuriousTimo/laserfoundry`, branch `maker-connect`): the accent
   `#93A682`, light and dark from the hub, the hub's fonts. The page then looks like Maker Connect inside as well.
   Needs a new pinned fork build (`scripts/laserfoundry-web.pin`, `scripts/laserfoundry-web.sh`).
3. **No iframe.** Load the wasm app straight into the hub's page so it shares the hub's CSS. Most work; touches how the
   fork is built and how it finds its files. Only if 1 and 2 are not enough.

Advice: 1 now, 2 when the inside should match too. Nothing here touches what goes to a laser.
