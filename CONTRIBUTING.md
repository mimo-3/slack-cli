# Contributing to `@mimo-3/slack-cli`

Thanks for your interest in contributing. This guide covers the workflow used in this repository.

## Prerequisites

- Node.js >= 20
- npm 10+ (the repo is locked with `package-lock.json`; please use npm, not pnpm/yarn, to avoid lockfile drift)

## Setup

```bash
git clone https://github.com/mimo-3/slack-cli.git
cd slack-cli
npm install
npm test           # run the full vitest suite
npm run check      # lint + format check via biome
npm run build      # tsc
```

## Development Workflow

This project is developed in small TDD cycles. Each pull request should:

1. **Pick one implementation target.** Keep PRs focused; if you find yourself doing two unrelated
   things, split them.
2. **Write a failing test first.** New behavior must be covered by a vitest spec under `tests/`.
3. **Make the test pass.** Implement just enough to turn the test green.
4. **Run the full CI suite locally.** `npm run check && npm run build && npm test`.
5. **Bump the version.** Edit `package.json` per semver:
   - **patch** for bug fixes and dependency bumps
   - **minor** for new commands or new options
   - **major** for breaking changes to existing CLI surface
6. **Open a PR.** A GitHub Release is automatically created when the PR is merged to `main`.

## Commit Style

- Keep each commit small (1–2 files when possible).
- Use a short, present-tense subject in either Japanese or English.
- Avoid noisy "wip" / "fix typo" commits — squash or amend before pushing.

## Pull Requests

- Fill in the PR template (summary + test plan).
- Make sure CI is green before requesting review.
- Link the related issue if any (`Closes #123`).

## Code Style

- Lint and format via [Biome](https://biomejs.dev/): `npm run check:fix`.
- TypeScript strict mode is on; prefer explicit types over `any` and avoid `as unknown as Foo` double casts.
- User-visible CLI strings are English. Error messages should suggest a remediation when possible.

## Reporting Bugs / Requesting Features

Please use the issue templates under [`.github/ISSUE_TEMPLATE/`](.github/ISSUE_TEMPLATE/). For
security-related reports, follow [SECURITY.md](SECURITY.md) instead of filing a public issue.

## License

By contributing, you agree that your contributions will be licensed under the project's
[MIT License](LICENSE).
