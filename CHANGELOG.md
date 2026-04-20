# Changelog

## [0.1.12](https://github.com/iOfficeAI/aionrs/compare/v0.1.11...v0.1.12) (2026-04-20)


### Features

* add output compaction for tool results ([#54](https://github.com/iOfficeAI/aionrs/issues/54)) ([63130c7](https://github.com/iOfficeAI/aionrs/commit/63130c70ead6dc30fb5244e49515bddc767c3c66))


### Documentation

* sync documentation with v0.1.8–v0.1.12 code changes ([#56](https://github.com/iOfficeAI/aionrs/issues/56)) ([436b09b](https://github.com/iOfficeAI/aionrs/commit/436b09b5a44997b0fb3b679b31ffe608b9f2ebf9))

## [0.1.11](https://github.com/iOfficeAI/aionrs/compare/v0.1.10...v0.1.11) (2026-04-17)


### Features

* **cli:** add team mode support with dynamic MCP server injection ([#50](https://github.com/iOfficeAI/aionrs/issues/50)) ([a16c9ee](https://github.com/iOfficeAI/aionrs/commit/a16c9eed2d64e679f23f18347fc02c423532298b))

## [0.1.10](https://github.com/iOfficeAI/aionrs/compare/v0.1.9...v0.1.10) (2026-04-16)


### Bug Fixes

* **openai:** handle empty function name in SSE deltas + add response dump ([#43](https://github.com/iOfficeAI/aionrs/issues/43)) ([d7ba0fa](https://github.com/iOfficeAI/aionrs/commit/d7ba0fabe48ea2a19cff42d8764ca5ccc1a3d608))

## [0.1.9](https://github.com/iOfficeAI/aionrs/compare/v0.1.8...v0.1.9) (2026-04-15)


### Features

* input token optimization — deferred tools, description truncation, prompt caching ([#41](https://github.com/iOfficeAI/aionrs/issues/41)) ([b20ce58](https://github.com/iOfficeAI/aionrs/commit/b20ce5813c94fa8c6a682ae8d88c7f17f42ec05a))

## [0.1.8](https://github.com/iOfficeAI/aionrs/compare/v0.1.7...v0.1.8) (2026-04-14)


### Features

* agent evolution - memory, compaction, plan mode, tool enhancement & file cache ([#32](https://github.com/iOfficeAI/aionrs/issues/32)) ([0b2a486](https://github.com/iOfficeAI/aionrs/commit/0b2a486e4e921d3b005307675c102a68d4b8f7ed))
* runtime config and capability discovery ([#36](https://github.com/iOfficeAI/aionrs/issues/36)) ([9539b54](https://github.com/iOfficeAI/aionrs/commit/9539b540c64f30ce6afcbfef65a078ab88913f50))


### Bug Fixes

* isolate sub-agent stdout to prevent JSON stream corruption ([#34](https://github.com/iOfficeAI/aionrs/issues/34)) ([6a7584a](https://github.com/iOfficeAI/aionrs/commit/6a7584abe9d0f5c85c36c01858d503cc72d9facd))


### Code Refactoring

* centralize platform-specific paths via app_config_dir() ([ad87748](https://github.com/iOfficeAI/aionrs/commit/ad87748edb299ff488c839630f065ccafc6e28dc))


### Documentation

* refactor AGENTS.md to focus on rules and conventions ([0f81cbc](https://github.com/iOfficeAI/aionrs/commit/0f81cbc644f8c9ed3cbe6af690bc23434feb6c0a))
* update file paths to reflect multi-crate workspace structure ([51b6cc7](https://github.com/iOfficeAI/aionrs/commit/51b6cc7bd29a51af7ad57aa8f87901e85005da42))

## [0.1.7](https://github.com/iOfficeAI/aionrs/compare/v0.1.6...v0.1.7) (2026-04-09)


### Bug Fixes

* **ci:** handle scoped release commit message in release-please workflow ([#22](https://github.com/iOfficeAI/aionrs/issues/22)) ([7222806](https://github.com/iOfficeAI/aionrs/commit/72228064a58d9a8ee410d37ad2380c8f84361cc9))

## [0.1.6](https://github.com/iOfficeAI/aionrs/compare/v0.1.5...v0.1.6) (2026-04-09)


### Bug Fixes

* **ci:** fix release_created typo and update Cargo.lock ([#18](https://github.com/iOfficeAI/aionrs/issues/18)) ([3964963](https://github.com/iOfficeAI/aionrs/commit/3964963f2d45849985c93e5f005cf59e6615573e))

## [0.1.5](https://github.com/iOfficeAI/aionrs/compare/v0.1.4...v0.1.5) (2026-04-09)


### Bug Fixes

* **ci:** fix release workflow to correctly build and upload GitHub Release assets ([#12](https://github.com/iOfficeAI/aionrs/issues/12)) ([997ec18](https://github.com/iOfficeAI/aionrs/commit/997ec18cbbd21ea2ef8eb19ff4cbf6280376a80c))

## [0.1.4](https://github.com/iOfficeAI/aionrs/compare/v0.1.3...v0.1.4) (2026-04-09)


### Bug Fixes

* **ci:** fix action versions and install cargo-audit ([#10](https://github.com/iOfficeAI/aionrs/issues/10)) ([8512765](https://github.com/iOfficeAI/aionrs/commit/85127654ce7afc4ec04b7e6a325d8470e0770175))

## [0.1.3](https://github.com/iOfficeAI/aionrs/compare/v0.1.2...v0.1.3) (2026-04-09)


### Features

* accept optional session ID in SessionManager::create and AgentEngine::init_session ([b5e50e8](https://github.com/iOfficeAI/aionrs/commit/b5e50e82ad8420cf603b3689ea4faba47df988b9))
* add --config-path flag and warn on config parse failure ([2f67ed8](https://github.com/iOfficeAI/aionrs/commit/2f67ed8bff7d1b585a74e112c39f86ebb9a7fba8))
* add --session-id flag and --resume support in json-stream mode ([6ecfa09](https://github.com/iOfficeAI/aionrs/commit/6ecfa094042c2cf1966758a105c7fd76db167516))
* add --version flag support for AionUi integration ([0d32f1f](https://github.com/iOfficeAI/aionrs/commit/0d32f1f21e72ac219eab638c4ca9e2391dd9f42b))
* add ProviderCompat configuration layer (Phase 0.1) ([cc4a315](https://github.com/iOfficeAI/aionrs/commit/cc4a31547283c9cfae1d7c279784e4a2a2e4ffd5))
* add session_id field to Ready protocol event ([f1025b5](https://github.com/iOfficeAI/aionrs/commit/f1025b567dddfd8d92e1afa40ab31fd438a85485))
* Bedrock schema sanitization via compat config (Phase 1.4) ([7802f19](https://github.com/iOfficeAI/aionrs/commit/7802f19709f015f1851a14943a1c2b71d1771f07))
* compat-driven message alternation, merging, and auto tool ID (Phase 1.1, 1.8) ([9bd5b3c](https://github.com/iOfficeAI/aionrs/commit/9bd5b3c692bafdd284a8179b6b29647c2bea381a))
* **compat:** add configurable api_path for chat completions endpoint ([ad8b6e9](https://github.com/iOfficeAI/aionrs/commit/ad8b6e949da7393dfa0d5cecf75449edeba98dbf))
* enhanced Bedrock error messages with actionable hints (Phase 2.1) ([a80d0ff](https://github.com/iOfficeAI/aionrs/commit/a80d0ff03e7c3a3559dc446d21a401b815460e89))
* initial commit of aionrs ([f8f3249](https://github.com/iOfficeAI/aionrs/commit/f8f3249acfcf595a2634d4ba37ae14993d365246))
* integrate ProviderCompat into config system (Phase 0.2) ([c0a4753](https://github.com/iOfficeAI/aionrs/commit/c0a47539eab824aea6c823455df3eafbef6f7016))
* OpenAI compat features - max_tokens field, message merging, orphan cleanup, dedup, strip patterns (Phase 1.2, 1.3, 1.5, 1.6, 1.7) ([c61896d](https://github.com/iOfficeAI/aionrs/commit/c61896d367abbfc31c64a5ae827599a5eee4e558))
* OpenAI reasoning model support (Phase 3.1) ([106108a](https://github.com/iOfficeAI/aionrs/commit/106108a46c06398c3f68d803a8528fad3b43b8d0))
* pass ProviderCompat to all providers (Phase 0.3) ([d9c6e1b](https://github.com/iOfficeAI/aionrs/commit/d9c6e1b0901732666b4fa96b747fe5af53929a99))
* session ID and resume support for JSON stream mode ([d36df5e](https://github.com/iOfficeAI/aionrs/commit/d36df5ed80855ba2fa2fc900a6bee2deda856869))
* skills system - named prompt snippets with tool orchestration ([#5](https://github.com/iOfficeAI/aionrs/issues/5)) ([4a5183f](https://github.com/iOfficeAI/aionrs/commit/4a5183fc7657ad756e751986f8c5c471346642cb))
* support custom provider aliases in configuration ([#2](https://github.com/iOfficeAI/aionrs/issues/2)) ([9fde728](https://github.com/iOfficeAI/aionrs/commit/9fde728f588ae0233179038984551c016d50919d))
* wire up skills system in main.rs and fix symlink traversal ([f93303c](https://github.com/iOfficeAI/aionrs/commit/f93303c9978fce867ee5361b97ee5fb4a4e2e31f))


### Bug Fixes

* **ci:** fix invalid workflow files (matrix.if + YAML syntax) ([#6](https://github.com/iOfficeAI/aionrs/issues/6)) ([4fd6de4](https://github.com/iOfficeAI/aionrs/commit/4fd6de49ad063d5e747b9fcff05d0f29b5535df3))
* **release:** bootstrap release-please for Cargo workspace ([#8](https://github.com/iOfficeAI/aionrs/issues/8)) ([18dd3e3](https://github.com/iOfficeAI/aionrs/commit/18dd3e32213ffabe022f05eaa9b16ec89ad04a76))


### Code Refactoring

* remove Claude branding, use AGENTS.md and AIONRS_* variables ([97dc25c](https://github.com/iOfficeAI/aionrs/commit/97dc25cabd3f4bc15e34d3a42da5dd42120e3bb2))
* split into Cargo workspace with fine-grained crates + CI/E2E ([#3](https://github.com/iOfficeAI/aionrs/issues/3)) ([a4537d9](https://github.com/iOfficeAI/aionrs/commit/a4537d944b3f3643ecb7db58c569f583edea7f97))


### Documentation

* add AGENTS.md with architecture principles and CLAUDE.md reference ([5dfeb89](https://github.com/iOfficeAI/aionrs/commit/5dfeb8990f6ba59363ad8c51eb3ba7738546f56f))
* document --session-id flag and session_id in Ready event ([510c141](https://github.com/iOfficeAI/aionrs/commit/510c141bf93b7a39ba6dda9cef9d9b8a2791f900))
* replace hardcoded ~/.config/aionrs paths with --config-path ([d94d518](https://github.com/iOfficeAI/aionrs/commit/d94d518ea12b0360a9f27e440fb7c0e08b0ffe93))
* update README with ProviderCompat layer and reasoning model support ([c831e21](https://github.com/iOfficeAI/aionrs/commit/c831e2119804f0e5bb2a080f9bef8c5df093dff3))

## Changelog

All notable changes to this project will be documented in this file.

See [Conventional Commits](https://conventionalcommits.org) for commit guidelines.
