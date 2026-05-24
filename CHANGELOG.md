# Changelog

All notable changes to this project will be documented in this file.

> This project is a fork of [urugus/slack-cli](https://github.com/urugus/slack-cli). Entries below are from the upstream project; changes since the fork are tracked in GitHub Releases.

## [0.22.0] - 2026-05-24

### Added
- `SECURITY.md` with a private vulnerability reporting policy and notes on legacy token storage
- `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1)
- Issue templates (bug report, feature request) and pull request template
- `repository`, `bugs`, `homepage` fields in `package.json`
- README sections for `invite`, `join`, `leave`, `members`, `send-ephemeral`, `reminder`,
  `bookmark`, and `users presence`; option tables updated for `send --blocks/--blocks-file`,
  `edit --blocks/--blocks-file/--file`, and `upload --format`
- README notes on legacy v0.x token storage and on the file-path trust boundary
- Stack-trace token masking in `command-wrapper` (redacts `xox*-…` patterns when `NODE_ENV=development`)

### Changed
- Regenerated `package-lock.json` to resolve known `axios` advisories (HIGH); `npm audit` now reports 0 vulnerabilities
- Hardened `claude-issue-triage` workflow against prompt injection by passing the issue body through
  `gh issue view` instead of embedding `${{ github.event.issue.body }}` directly in the prompt
- Replaced `ubuntu-slim` with `ubuntu-latest` across all GitHub Actions workflows
- Translated the lone Japanese error message in `bookmark` to English for consistency
- Replaced upstream-derived `dev_kiban_jira` channel names in tests with the generic `engineering`
- Migrated `biome.json` `$schema` to match the installed Biome CLI version

## [0.21.0] - 2026-05-24

### Added
- `send --blocks <json>` and `send --blocks-file <file>` to post Block Kit messages
- `edit --blocks <json>`, `edit --blocks-file <file>`, and `edit --file <file>` to update messages
  with Block Kit content or content read from a file
- `upload --format <table|simple|json>` for machine-readable upload output (returns `file_id`,
  `permalink`, etc.)
- `history --format json` output now includes `user_id` for each message
- `upload` now returns the underlying Slack file metadata (file_id, permalink, etc.) regardless of format

## [0.20.22] - 2026-05-24

### Changed
- Renamed package from `@urugus/slack-cli` to `@mimo-3/slack-cli` to publish under the fork's npm scope
- Added fork credit and Slack trademark disclaimer to README
- Added fork author to LICENSE alongside the original copyright holder

## [0.4.4] - 2026-02-22

### Changed
- Replace generic `Error` with `ConfigurationError` in `TokenCryptoService` (encrypt/decrypt failures)
- Replace generic `Error` with `ValidationError` in `TokenCryptoService` (invalid data format validation)
- Replace generic `Error` with `ConfigurationError` in `ProfileConfigManager` (profile not found, invalid config)
- Replace generic `Error` with `ValidationError` in `createOptionParser` (validation failures)
- Replace generic `Error` with `ApiError` in `ChannelResolver` (channel not found errors)

### Added
- Error type verification tests for `TokenCryptoService`, `ProfileConfigManager`, `ChannelResolver`, and `createOptionParser`

## [0.4.3] - 2026-02-22

### Changed
- Replace `any` types with proper TypeScript types in `MessageFormatterOptions` (`Channel`, `Message[]`)
- Replace `any` type in `JsonMessageFormatter` output with explicit `MessageJsonOutput` interface
- Replace `as any` cast in `MessageOperations.listScheduledMessages` with `ChatScheduledMessagesListArguments`
- Replace `Promise<any>` return type in `ChannelOperations.fetchLatestMessage` with `Promise<Message | null>`
- Replace `TOutput = any` default in `JsonFormatter` base class with `TOutput = unknown`

### Added
- New test file for message formatters (`tests/utils/formatters/message-formatters.test.ts`)

## [0.4.2] - 2026-02-22

### Changed
- Consolidated duplicate configuration management systems into a single `ProfileConfigManager`
- Integrated `TokenCryptoService` into `ProfileConfigManager` for automatic token encryption at rest
- Tokens are now encrypted (AES-256-CBC) when saved and decrypted when read
- Existing plaintext tokens are still readable for backward compatibility
- Old config format migration now also encrypts tokens

### Removed
- Removed unused `ConfigFileManager` class (`src/utils/config/config-file-manager.ts`)
- Removed unused `ProfileManager` class (`src/utils/config/profile-manager.ts`)
- Removed duplicate test file (`tests/utils/config.test.ts`)

## [0.2.1] - 2025-06-23

### Fixed
- Fixed unread message detection for channels where unread_count is 0 but messages exist after last_read timestamp
- Always check messages after last_read timestamp for accurate unread count
- Improved reliability of unread message detection for active channels

## [0.2.0] - 2025-06-23

### Changed
- Major version bump for improved unread message detection

## [0.1.9] - 2025-06-22

### Fixed
- Improved unread message detection using last_read timestamp

## [0.1.8] - 2025-06-22

### Changed
- Refactored code organization with separation of concerns

## [0.1.7] - 2025-06-22

### Fixed
- Resolved rate limiting issues with `slack unread` command
- Disabled WebClient automatic retries to handle rate limits manually
- Changed to use users.conversations API for more efficient unread retrieval
- Added fallback mechanism for better compatibility

## [0.1.6] - 2025-06-22

### Added
- Channel resolver abstraction for better code reusability
- Output formatter abstraction for flexible display formats
- Rate limiting configuration with exponential backoff
- Better error messages with channel suggestions

### Changed
- Improved rate limiting handling to prevent infinite retry loops
- Extracted magic numbers into configuration constants
- Refactored SlackApiClient to reduce complexity

### Fixed
- Fixed infinite loop when hitting Slack API rate limits
- Replaced all `any` types with proper type definitions
- Fixed CommonJS compatibility issues with p-limit

## [0.1.5] - Previous releases

### Added
- Initial implementation of Slack CLI
- Support for sending messages
- Channel listing functionality
- Message history retrieval
- Unread message tracking
- Multi-profile support