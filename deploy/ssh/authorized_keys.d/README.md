Public keys authorized to SSH into the Blink Hetzner server.

One `.pub` file per machine. Filename = lowercase hostname (e.g. `laptop.pub`,
`desktop.pub`). Files here are picked up by `../sync-authorized-keys.sh`.

Do not commit private keys. Filenames without extensions (`id_ed25519`, etc.)
are blocked by the repo's `.gitignore`.
