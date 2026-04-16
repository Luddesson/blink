# Blink SSH setup

Lets every authorized machine reach the Hetzner server with a plain `ssh blink`
— without anyone ever committing a private key.

## How it works

- **`blink-ssh-config`** — shared SSH config (`Host blink`). Included by each
  machine's `~/.ssh/config`. Points `IdentityFile` at `~/.ssh/blink_hetzner`.
- **`authorized_keys.d/<machine>.pub`** — one public key per machine that should
  have access. Committed to the repo.
- **`setup.ps1`** (Windows) — one-time per machine. Generates a private key in
  `~/.ssh/blink_hetzner` if missing, fixes ACLs, includes the shared config, and
  drops the machine's public key into `authorized_keys.d/`.
- **`sync-authorized-keys.sh`** — pushes all `authorized_keys.d/*.pub` into the
  server's `/root/.ssh/authorized_keys` inside a managed block. Run from any
  machine that already has access.

## Adding a new machine

On the new machine:
```powershell
cd <repo>
.\deploy\ssh\setup.ps1
git add deploy/ssh/authorized_keys.d/*.pub
git commit -m "ssh: authorize <machine>"
git push
```

From an already-authorized machine:
```bash
git pull
bash deploy/ssh/sync-authorized-keys.sh
```

Then on the new machine: `ssh blink` just works.

## Revoking a machine

Delete its `.pub` file, commit, push, run `sync-authorized-keys.sh` from an
authorized machine. The old key disappears from the server.

## Notes

- Private keys (`~/.ssh/blink_hetzner`) **never** leave the machine. The repo
  only holds public keys.
- The sync script preserves any authorized_keys entries outside the managed
  block (e.g. manual emergency keys).
- No passphrase on generated keys by default — add one with `ssh-keygen -p` if
  you want one; `ssh-agent` or Windows OpenSSH agent handles it from there.
