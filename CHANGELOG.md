# Changelog

## [1.1.0](https://github.com/VitruvianSoftware/vitruvian-core/compare/homelab-v1.0.3...homelab-v1.1.0) (2026-07-10)


### Features

* **build:** close the inter-app visibility firewall + conformance guard ([#82](https://github.com/VitruvianSoftware/vitruvian-core/issues/82)) ([#496](https://github.com/VitruvianSoftware/vitruvian-core/issues/496)) ([30e8a09](https://github.com/VitruvianSoftware/vitruvian-core/commit/30e8a09bfb2c19b1602e522f222ea518ca8e210a))
* **deploy:** per-app metadata catalog + reusable _deploy-cloud-run.yaml + tabula de-race ([#500](https://github.com/VitruvianSoftware/vitruvian-core/issues/500), [#499](https://github.com/VitruvianSoftware/vitruvian-core/issues/499)) ([#511](https://github.com/VitruvianSoftware/vitruvian-core/issues/511)) ([d546651](https://github.com/VitruvianSoftware/vitruvian-core/commit/d5466512896c9aa691cdff8e1d3016798cb3cd88))
* **gcp-bootstrap:** trigger release ([550e1df](https://github.com/VitruvianSoftware/vitruvian-core/commit/550e1dffdb3525781923eb4f3369050dd77c5aec))
* **gcp-org:** trigger release ([3bb4b9e](https://github.com/VitruvianSoftware/vitruvian-core/commit/3bb4b9efab7fa9a8c59a9d68966d6f4c252fa89f))
* **org-folders:** trigger release ([588230c](https://github.com/VitruvianSoftware/vitruvian-core/commit/588230c68cc864fa77efd6324dc40c511e1ba0c7))
* **pulumi/library:** integrate Go modules into the Bazel workspace ([b95e735](https://github.com/VitruvianSoftware/vitruvian-core/commit/b95e735574f373a9f08a31acb453ae6508ccc0b3))


### Bug Fixes

* **go/logging:** grant bucketWriter for log-bucket destination ([#63](https://github.com/VitruvianSoftware/vitruvian-core/issues/63)) ([1388d02](https://github.com/VitruvianSoftware/vitruvian-core/commit/1388d02c2314a775443713c0ad070bc5afd44826))
* **license:** enforce MIT + VitruvianSoftware (content gate) and relicense ([#477](https://github.com/VitruvianSoftware/vitruvian-core/issues/477)) ([639aaa0](https://github.com/VitruvianSoftware/vitruvian-core/commit/639aaa0750e9882b1719ac4c77c069a8b351e835)), closes [#457](https://github.com/VitruvianSoftware/vitruvian-core/issues/457)
* **pulumi/library:** correct TS BUILD load path; revert devx/homelab go.mod ([5f125db](https://github.com/VitruvianSoftware/vitruvian-core/commit/5f125db6885a95a38886c8d2b0d618de1e7c5e9b))

## [1.0.3](https://github.com/VitruvianSoftware/homelab/compare/v1.0.2...v1.0.3) (2026-04-27)


### Bug Fixes

* dynamically select healthy server node for cluster join ([61c9b4f](https://github.com/VitruvianSoftware/homelab/commit/61c9b4fc0b4841ab807aa5c80e2362a8189679db))

## [1.0.2](https://github.com/VitruvianSoftware/homelab/compare/v1.0.1...v1.0.2) (2026-04-13)


### Bug Fixes

* inject release automation token into pipeline ([deee977](https://github.com/VitruvianSoftware/homelab/commit/deee9778395aa1dfb31b4682f7043664fdaeb916))

## [1.0.1](https://github.com/VitruvianSoftware/homelab/compare/v1.0.0...v1.0.1) (2026-04-13)


### Bug Fixes

* trigger release for homebrew configuration ([87d46fa](https://github.com/VitruvianSoftware/homelab/commit/87d46fa2443c0f41b9c580551f989b73a8abcedb))

## 1.0.0 (2026-04-13)


### Features

* migrate to release-please for automated versioning ([#9](https://github.com/VitruvianSoftware/homelab/issues/9)) ([eced635](https://github.com/VitruvianSoftware/homelab/commit/eced635df64f68acd790536c8613dfbb9a8442f5))
* milestone 1 - foundation hardening ([#2](https://github.com/VitruvianSoftware/homelab/issues/2)) ([4689e8f](https://github.com/VitruvianSoftware/homelab/commit/4689e8fdda2b33fdb8e026b00d6e109fb4cf40a2))
* milestone 2 - parallel provisioning & prereqs ([#3](https://github.com/VitruvianSoftware/homelab/issues/3)) ([132a51f](https://github.com/VitruvianSoftware/homelab/commit/132a51f2e3845256bc8eb7b6dad907ac1823f010))
* milestones 3, 4, 5 - testing, CI/CD, advanced features ([#4](https://github.com/VitruvianSoftware/homelab/issues/4)) ([464cab5](https://github.com/VitruvianSoftware/homelab/commit/464cab5ab86f003bb88c12ced23ca20739bca9d0))
* Mise migration ([#13](https://github.com/VitruvianSoftware/homelab/issues/13)) ([11385f4](https://github.com/VitruvianSoftware/homelab/commit/11385f45dfeef2ea0a9819ae5f7aad912507a093))
* scaffold Go CLI project structure ([#1](https://github.com/VitruvianSoftware/homelab/issues/1)) ([5f43ae2](https://github.com/VitruvianSoftware/homelab/commit/5f43ae2bd542efde5ee5920da06d1525ec60c7ed))


### Bug Fixes

* **doctor:** check multiple paths for socket_vmnet ([#8](https://github.com/VitruvianSoftware/homelab/issues/8)) ([bbae0af](https://github.com/VitruvianSoftware/homelab/commit/bbae0af39dcf10280ebf6cbb144447d377ca2d94))
* Formatting in homelab CLI diagram ([#11](https://github.com/VitruvianSoftware/homelab/issues/11)) ([4a48ee1](https://github.com/VitruvianSoftware/homelab/commit/4a48ee137a236d1e2cd65f5298daa605e404e961))
* golangci-lint Go version mismatch in CI ([#5](https://github.com/VitruvianSoftware/homelab/issues/5)) ([70edcb6](https://github.com/VitruvianSoftware/homelab/commit/70edcb6a0c163debd7a9fe42715a162011477cd5))
* MacOS grep ([#7](https://github.com/VitruvianSoftware/homelab/issues/7)) ([84e95fa](https://github.com/VitruvianSoftware/homelab/commit/84e95fa88438229192b1ef2590296e5ff3fd82f9))
* replace grep -P with awk for POSIX compatibility ([#6](https://github.com/VitruvianSoftware/homelab/issues/6)) ([9edc8e3](https://github.com/VitruvianSoftware/homelab/commit/9edc8e39791c688af927f9a7c1ccc6e837d294ff))
