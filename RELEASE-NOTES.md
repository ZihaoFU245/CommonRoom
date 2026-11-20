# Commonroom 0.3.1

- Keep the chat header and sidebar anchored as the mobile keyboard opens.
  The composer stays above the keyboard, and its upload and sticker buttons
  stay in place as the input grows.
- Preserve scroll position when loading older messages during a touch scroll.
  Browser Back and Forward restore screens directly, and rapid room switches
  skip stalled or overlapping animations.
- Keep the message actions menu open after a long press. Holding a sticker
  adds its collection action to that menu and shows any ownership charge.
  Stickers enlarge slightly while pressed.
- Swipe a message in either direction to reply. The arrow points toward the
  message, and supported devices receive a short vibration on release.

Upgrades from 0.3.0 can use existing data directly. See [Data](docs/data.md)
for compatibility and migration details.
