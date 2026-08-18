# slack-cli

Slack をコマンドラインから操作する。メッセージの送信・履歴の取得・検索・ファイルのアップロード・
下書きや予約投稿の管理まで、26 コマンドを 1 バイナリで持つ。Block Kit にも対応している。

TypeScript 版（[slack-cli-ts](https://github.com/mimo-3/slack-cli-ts)）を Rust に書き直したもの。
設定ファイルとトークンの保存形式は TypeScript 版と互換なので、使っていた人は再ログインなしで乗り換えられる。
Node.js も不要になった。

## インストール

```bash
brew install mimo-3/tap/slack-cli
```

macOS（Intel / Apple Silicon）と Linux（x86_64）のビルド済みバイナリを配っているので、
Rust も Node.js も要らない。ソースから入れるなら:

```bash
cargo install --path .
```

## 使い始める

```bash
# トークンを保存する（argv に残さないので stdin から渡す）
printf '%s' "$SLACK_TOKEN" | slack-cli config set --token-stdin

# 疎通確認
slack-cli auth test

# 送ってみる
slack-cli send -c '#general' -m 'hello'
```

## 何ができるか

| やりたいこと | コマンド |
| --- | --- |
| メッセージを送る・予約する・直す・消す | `send` `send-ephemeral` `edit` `delete` |
| 履歴と未読を読む | `history` `unread` |
| チャンネルを調べる・入る・出る・招く | `channels` `channel` `join` `leave` `invite` `members` |
| 検索する | `search` |
| ユーザーとユーザーグループを調べる | `users` `usergroups` |
| リアクションとピンを操作する | `reaction` `pin` |
| ファイルとキャンバスを扱う | `upload` `download` `canvas` `bookmark` |
| 下書き・予約投稿・リマインダーを管理する | `draft` `scheduled` `reminder` |
| 設定とトークンを管理する | `config` `auth` |

各コマンドのオプションは `slack-cli <コマンド> --help` で見られる。

グローバルオプションはどのサブコマンドの後ろにも置ける。
`--token` `--profile` `--format`（table・json・yaml・csv・tsv・id-only）`--json`
`--dry-run` `--no-color`。

## 保存先

| 対象 | パス |
| --- | --- |
| 設定 | `~/.slack-cli/config.json`（ディレクトリ 0700 / ファイル 0600） |
| 暗号鍵 | `~/.slack-cli-secrets/master.key`（旧配置 `~/.slack-cli/master.key` から自動で移る） |

トークンは AES-256-GCM で暗号化して保存する。旧 AES-256-CBC 形式は読めるだけ読んで、
読んだ時点で新形式に書き直す。

## TypeScript 版からの変更点

コマンド名・オプション名・JSON 出力の構造・終了コードは変えていない。
一方で、TypeScript 版にあった不具合は直した。主なものは 4 つ。

- `--limit` に数字以外を渡すと、そのまま Slack API に流れて空の結果が返っていた。今は弾く
- レート制限をエラーメッセージの文字列一致で判定していた。今は HTTP 429 と `Retry-After` を見る
- `join` `leave` `invite` の成功メッセージだけエスケープ処理が抜けていた
- `download --url` が宛先を確かめずにトークンを付けていた。今は Slack のドメインだけに送る

日本語や絵文字を含む一覧の列ズレも直っている。
変更したものの全一覧は [docs/spec/01-porting-policy.md](docs/spec/01-porting-policy.md) にある。

## 開発

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

- `src/cli/` — コマンド定義と各コマンドの処理。HTTP は直接触らない
- `src/client/` — Slack Web API クライアント。認証・リトライ・ページング
- `src/config/` — 設定ファイルの読み書きとトークンの暗号化
- `src/output/` — 出力の書き出し
- `src/error.rs` — エラーの種類と終了コード（エラーは一律 1。種別は `code()` で取れる）

コマンドを足す手順は `src/cli/mod.rs` の冒頭コメントにある。
移植元の仕様は `docs/spec/` に置いてある。
