# Contributing to Nexora

Thanks for wanting to help! Bug reports, ideas and pull requests are all welcome — including small ones. If you're a beginner, this is a friendly place to make your first contribution.

## How to contribute

1. Open an issue first for anything non-trivial, so we can discuss the approach.
2. Fork, create a branch, make your change.
3. Before opening the PR, make sure the checks pass locally:
   ```bash
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   ```
4. Keep commits and code comments in English.

### Faster builds (optional)

Release builds use thin LTO, so they are already fast (~15–20s incremental).
To shave link time further, install [mold](https://github.com/rui314/mold) and
create `.cargo/config.toml` locally (it is git-ignored):

```toml
[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=mold"]
```

For the smallest release artifact (fat LTO, `panic = abort`), build the `dist`
profile: `cargo build --profile dist`.

### Guidelines

- **Stay light.** Nexora's whole identity is being ridiculously lightweight. New dependencies need a strong justification; anything that adds a background thread, a busy loop or memory overhead will be scrutinized.
- **Be honest in the UI.** When a platform can't do something (e.g. anti-capture on GNOME), we say so plainly instead of faking it.
- **Wayland first, X11 still works.** Don't break either.

## Contributor License Agreement (CLA)

By submitting a contribution (pull request, patch, or code in any other form) to this repository, you agree that:

1. **You own your contribution** — it is your original work, or you have the right to submit it under these terms.
2. **License grant** — you grant Miguel Lopes Delmondes ("the project owner") a perpetual, worldwide, non-exclusive, irrevocable, royalty-free, transferable license, with the right to sublicense, to use, reproduce, modify, distribute, publicly display, publicly perform, and create derivative works of your contribution, and to **distribute it under any license terms**, including licenses other than the one this project currently uses.
3. **You keep your rights** — this is a license, not a copyright transfer. You remain free to use your own contribution however you like.
4. **Outbound license** — your contribution is distributed to everyone else under the project's current license (AGPL-3.0), and every released version stays available under the license it was released with.
5. **No warranty** — contributions are provided as-is, without warranties of any kind.

This CLA exists so the project can, in the future, change the license of *new* versions or offer optional commercial services alongside the open source client (the "open core" model used by projects like GitLab and Grafana). Nothing already released ever stops being open source.

If you do not agree with the CLA, please don't submit code — issues and ideas are still very welcome.

*Note: this CLA was written in good faith by a developer, not a law firm. If you spot a problem with it, open an issue.*
