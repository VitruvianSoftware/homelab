# Contributing to homelab

## ⚠️ This is a read-only mirror — how contributions work

`homelab` is developed in the **[VitruvianSoftware/vitruvian-core](https://github.com/VitruvianSoftware/vitruvian-core)** monorepo (the single source of truth). **This repository is a read-only mirror** kept in sync by [Copybara](https://github.com/google/copybara) — you cannot merge directly here.

You contribute by opening a PR against this mirror; it is **imported back into the monorepo** for review:

1. **Open a PR** against `VitruvianSoftware/homelab`.
2. **Sign the CLA** (first PR only): post this exact comment on your PR — `I have read the CLA Document and I hereby sign the CLA` (see [CLA.md](./CLA.md)). The CLA check must be green.
3. **A maintainer applies the `import-to-monorepo` label.** Only labelled, CLA-signed PRs are imported.
4. **The monorepo imports your PR automatically** (within ~15 min — a scheduled job), opening a PR in `vitruvian-core` under `homelab/` **with you as the author**, where the full monorepo CI + review run.
5. **A maintainer merges the monorepo PR;** your mirror PR is then auto-commented and **auto-closed**, and your change reflects back here on the next export.

So: *open PR → sign CLA → maintainer labels → auto-import → review/merge in the monorepo → mirror PR auto-closes.* You never push to the monorepo directly. Keep changes scoped to this repo's content; merges happen in the monorepo now, not in this mirror.

## Local development

`homelab` is a Go CLI. Clone the mirror, build with the repo's standard Go
toolchain, and run its tests before opening a PR. All review and merge happens in
the monorepo (see above) — this mirror only accepts PRs for import.
