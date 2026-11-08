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
printf '%s' "$SLACK_TOKEN" | slack-cli config set --profile work --token-stdin

# 疎通確認
slack-cli auth test --profile work

# 送ってみる
slack-cli send -c '#general' -m 'hello' --profile work
```

既定プロファイルは任意。最初の `config set` では既定にならない。
`slack-cli config use work` で既定を設定し、`config use --none` で解除できる。
`config current` で確認し、`config profiles` で登録済みの名前を一覧できる。
認証は `--token`、`SLACK_CLI_TOKEN`、`--profile <名前>`、設定済みの既定の順に解決する。
どれもないときは API に通信する前にエラーになる。`--dry-run` でも同じチェックを行う。
`config set`・`get`・`clear` は `--profile` または既定を使う。
既定を削除しても別のプロファイルには繰り上がらない。`default` は通常の名前として扱う。
既存の `defaultProfile` は、対象が存在すれば既定として使う。

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

## 本文と画像をまとめて送る

```bash
# 本文と画像を1つのメッセージにする。--image は繰り返せる
slack-cli send -c '#general' -m '確認をお願いします' --image screenshot.png --profile work
slack-cli send -c '#general' --blocks-file body.json --image first.jpg --image second.webp --image-alt '画面の比較' --profile work

# どこにも共有せずにアップロードする。出力の files[].id を後から利用できる
slack-cli upload -f screenshot.png --json --profile work

# ファイルと blocks の本文をまとめてチャンネルへ投稿する
slack-cli upload -c '#general' -f report.csv --blocks-file body.json --profile work
```

`upload` は `--channel` を省略すると非公開アップロードになる。
`--thread`・`--message`・`--blocks`・`--blocks-file` には `--channel` が必要。
`--blocks` と `--blocks-file` は JSON 配列を受け取り、互いに併用できない。`--message` と blocks も併用できない。
JSON と table の出力にはファイル ID が含まれる。

`send --image` は png・jpg・jpeg・gif・webp を受け取る（拡張子の大文字・小文字は区別しない）。
各画像を非公開でアップロードし、`files.info` の `mimetype` が埋まるまで1.5秒間隔で最大30秒待つ。
`files:write` に加えて `files:read` が必要。
指定済みの blocks の末尾に画像を追加する。blocks がなければ本文を mrkdwn の section にする。
画像だけでも送れる。代替テキストはファイル名で、`--image-alt` を指定すると全画像に同じ文言を使う。
`invalid_blocks` の応答には1.5秒間隔で最大3回再試行する。

`send --image` と `--at` / `--after` の併用はアップロード前にエラーになる。
`--dry-run` は画像の検証と送信内容の表示だけを行い、アップロード・処理待ち・投稿を行わない。
途中で失敗した場合、完了済みの非公開ファイルは残る。自動削除は行わない。

## 保存先

| 対象 | パス |
| --- | --- |
| 設定 | `~/.slack-cli/config.json`（ディレクトリ 0700 / ファイル 0600） |
| 暗号鍵 | `~/.slack-cli-secrets/master.key`（旧配置 `~/.slack-cli/master.key` から自動で移る） |

トークンは AES-256-GCM で暗号化して保存する。旧 AES-256-CBC 形式は読めるだけ読んで、
読んだ時点で新形式に書き直す。

## TypeScript 版からの変更点

既定プロファイルを任意にしたため、認証と config コマンドの一部は TypeScript 版と異なる。
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
