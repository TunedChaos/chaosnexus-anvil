<img src="./assets/banner.png" alt="ChaosNexus Anvil Banner" />

# ChaosNexus Anvil

Rust MCP host engine and sandboxed Rhai scripting runtime for the ChaosNexus platform.

> **Status:** early public alpha launch (pre-1.0).

- **Docs:** [chaosnexus.ai](https://chaosnexus.ai)
- **Contribute:** [codeberg.org/TunedChaos/chaosnexus-anvil](https://codeberg.org/TunedChaos/chaosnexus-anvil) (primary)
- **Mirror:** [github.com/TunedChaos/chaosnexus-anvil](https://github.com/TunedChaos/chaosnexus-anvil) (read-only; Sponsors)

Please open issues and pull requests on **Codeberg**. GitHub PRs are not accepted on the mirror.

## AI assistance

Some code in this project was generated with assistance from AI. Humans directed architecture, review, and maintenance. See [AI_ASSISTANCE.md](AI_ASSISTANCE.md).

## Quick start

```bash
cargo build --release
cargo run
```

Plugin scripts default to `../chaosnexus-scripts` relative to the Anvil working directory. In the monorepo that is the shared [`chaosnexus-scripts/`](https://codeberg.org/TunedChaos/chaosnexus-scripts) tree. Standalone clones should point `scripts_dir` in your host TOML at a local plugins checkout.

## License

AGPL-3.0-or-later. Commercial licensing: [chaosnexus.ai/guide/licensing](https://chaosnexus.ai/guide/licensing).
