# chaosnexus-anvil/packaging/aur/README.md
# AUR package draft for ChaosNexus Anvil (not submitted)

## Status

This `PKGBUILD` is a **draft**. It is **not** published to the Arch User Repository
and is not installable via `paru` / `yay` / official repos yet.

## Docs embedding

Release and AUR packaging intentionally **do not** embed Codex documentation.
Users fetch/index docs themselves (Codex `fetch` / config), matching the
release-build policy.

## Local makepkg (manual)

1. Build Anvil release binaries from the monorepo:
   ```bash
   just anvil-release
   ```
2. From `chaosnexus-anvil/packaging/aur/`:
   ```bash
   ln -sf ../../../../artifacts/anvil/chaosnexus-anvil-0.1.0-x86_64-unknown-linux-gnu \
     chaosnexus-anvil-0.1.0-x86_64-unknown-linux-gnu
   makepkg -si
   ```
3. Before any future AUR submission:
   ```bash
   makepkg --printsrcinfo > .SRCINFO
   ```

## AUR publish

Deferred until public Codeberg Releases exist for `chaosnexus-anvil`.
