<img src="./assets/banner.png" alt="ChaosNexus Anvil Banner" />

# ChaosNexus Anvil

Rust MCP host engine and sandboxed Rhai scripting runtime for the ChaosNexus platform.

> **Status:** early public alpha launch (pre-1.0).

- **Docs:** [chaosnexus.ai](https://chaosnexus.ai)
- **Contribute:** [github.com/TunedChaos/chaosnexus-anvil](https://github.com/TunedChaos/chaosnexus-anvil)
- **Sponsors:** [github.com/sponsors/TunedChaos](https://github.com/sponsors/TunedChaos)
GitHub is the primary public host. Please open issues and pull requests on **GitHub**.

## AI assistance

Some code in this project was generated with assistance from AI. Humans directed architecture, review, and maintenance. See [AI_ASSISTANCE.md](AI_ASSISTANCE.md).

## Quick start

```bash
cargo build --release
cargo run
```

Plugin scripts default to `../chaosnexus-scripts` relative to the Anvil working directory. In the workspace that is the shared [`chaosnexus-scripts/`](https://github.com/TunedChaos/chaosnexus-scripts) tree. Standalone clones should point `scripts_dir` in your host TOML at a local plugins checkout.

## Support

ChaosNexus is maintained by a solo developer. If it helps you, consider sponsoring — it funds continued OSS work, not a support SLA:

**[GitHub Sponsors — TunedChaos](https://github.com/sponsors/TunedChaos)**

File bugs on [chaosnexus-suite Issues](https://github.com/TunedChaos/chaosnexus-suite/issues) (pick a Component).


## License

AGPL-3.0-or-later. Commercial licensing: [chaosnexus.ai/guide/licensing](https://chaosnexus.ai/guide/licensing).
