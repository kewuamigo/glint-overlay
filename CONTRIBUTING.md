# Contributing

Thanks for helping with Glint. This repo is the public product tree — build it, fix it, open a PR.

## Before you start

- Windows 10/11
- Node.js 22+
- Rust 1.85+
- MSVC Build Tools recommended

```powershell
npx pnpm@10.12.1 install
.\build-all.ps1
```

Run the launcher as Administrator: `.\target\release\glint-launcher.exe`.

Guides: [docs/development.md](docs/development.md), [docs/TESTING.md](docs/TESTING.md), [docs/plugin-development.md](docs/plugin-development.md).

## What to test

- `cargo test --workspace` for Rust changes
- Relevant package tests / smoke scripts when you touch cloud sync or save packing
- Manual attach on at least one title if you change overlay, input, or CEF hosting

## Pull requests

1. Keep PRs focused — one problem or feature per PR when possible.
2. Describe **what** changed and **how you verified** it.
3. Do not add editor agent configs, personal tooling, or scratch logs.
4. Match existing code style; avoid drive-by refactors.

## CI notes

Full Windows overlay builds need CEF binaries (`scripts/fetch-cef.ps1`) and native toolchains. GitHub Actions may not reproduce a local Admin attach session. The `release` workflow packages installers when tags are cut and CEF fetch succeeds — treat failed packaging jobs as environment/setup issues unless the same failure reproduces locally.

## Code of conduct (short)

Be respectful. No harassment. Assume good intent on reviews.

## Questions

Prefer GitHub Issues for bugs and design questions. Include OS build, GPU API (DX11/12/Vulkan/…), and a short repro when reporting overlay issues.
