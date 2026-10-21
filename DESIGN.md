# Gravevault: design (draft for review)

**Backups you've proven you can restore.**

## Scope

- **Git:** absorbs `~/.local/bin/cybercore-gitlab-backup` (append-only
  GitHub → GitLab, LFS from local checkouts).
- **Encrypted snapshots:** of what matters on blackbox (`~/Vaults`,
  `~/.config`, the user-service subvolume, `~/Documents/Blackbox`, the
  Cybernote/CyberVault data) via **rustic** (Rust, restic-compatible repo
  format: deduplicated, encrypted, verifiable).
- **Restore drills:** on a schedule, restore a random sample of files (and
  periodically a full vault) into a scratch directory, verify hashes, and
  report through **Warnflare's webhook**. A backup counts only if its last
  drill passed.
- **Retention and pruning:** per source, keep-last/daily/weekly/monthly.
- **Status:** a `last.json` per job (Warnflare freshness rules watch them),
  plus the Nightwatch alerts tab.

## Milestones

1. Port the GitLab mirror to Rust (same append-only semantics, retry,
   LFS).
2. rustic snapshots to the first destination, with retention.
3. Restore drills + Warnflare reporting.
4. A second destination (3-2-1).

## Decision needed: where do snapshots go?

Codex's earlier attempt stalled because Google blocked rclone. Options:

- **darkbox** over the tailnet (free and fast, but in your house).
- **Backblaze B2** or **Cloudflare R2** (S3-compatible, cents per GB,
  off-site). You already use Cloudflare.
- **MEGA** (MEGAsync is already running).

Recommended: **darkbox + Cloudflare R2**, which gives two copies, one
off-site.
