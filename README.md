# ⚰️ Gravevault

**Backups you've proven you can restore.**

Gravevault keeps everything that matters on your machines backed up, and it doesn't count a backup until a restore drill has proved it. Failures and recoveries go to [Warnflare](https://github.com/cybercoretech/warnflare) (nightwatchd's alert rules), so a broken backup reaches your phone instead of sitting silently in a log.

## 🪦 Milestone 1: the append-only git mirror

`gravevault mirror` copies every repository of a GitHub owner into a private GitLab group:

- **Append-only:** never `--force`, never `--prune`. A branch deleted on GitHub stays on GitLab. A force-pushed branch keeps its old history, and the new tip is saved beside it as `rewritten/<branch>/<UTC>`.
- **New repos** get a private GitLab project automatically.
- **Git LFS** objects are pushed from local checkouts you list, never fetched from GitHub, to save its LFS bandwidth quota.
- **Resilient:** retries GitLab hiccups, holds a run lock, and writes its state to `~/.local/state/gravevault/git-mirror.json`, which a Warnflare freshness rule can watch.
- **Reports:** a failed run raises `crit`, and the first clean run afterwards raises `info` (recovered).

It replaces the `cybercore-gitlab-backup` bash script, with the same semantics. Auth comes from the `gh` and `glab` credential helpers you already have.

## ⚙️ Configuration

`~/.config/gravevault/config.toml`:

```toml
[git_mirror]
github_owner = "cybercoretech"
gitlab_group = "cybercoretech-backup"

[git_mirror.lfs_sources]
NeonCoffinAnimations = "~/Animations/NCV_Animations"

[warnflare]
url = "http://127.0.0.1:8769/v1/events"
token_file = "~/.config/nightwatch/warnflare-token"
```

The webhook token reaches curl through a config on **stdin**, never on the command line, where `/proc/<pid>/cmdline` would expose it.

## 🧪 Development

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

The tests include a real-git run proving a force-push upstream never erases the mirror, and a capturing HTTP server proving the Warnflare request arrives intact.

## 🗺️ Roadmap

See [DESIGN.md](DESIGN.md):

1. ✅ git mirror
2. Encrypted `rustic` snapshots to **darkbox + Cloudflare R2**, with retention.
3. Scheduled **restore drills** reported to Warnflare.
4. 3-2-1 verification.

MIT licensed.
