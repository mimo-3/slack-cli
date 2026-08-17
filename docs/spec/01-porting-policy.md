# 移植方針 — TS版の不具合・設計ミスをRust版でどう扱うか

対象: `/Users/mimo/organizations/open-source/slack-cli`（`@mimo-3/slack-cli` v0.24.1、TypeScript）
移植先: `/Users/mimo/organizations/open-source/slack-cli-rs`（Rust）

本書は `docs/spec/` 配下の11本の仕様書（`00-rust-skeleton.md` / `common-infra.md` / `common-client.md` /
`common-format.md` / `cmd-config.md` / `cmd-channel.md` / `cmd-message.md` / `cmd-history.md` /
`cmd-file.md` / `cmd-defer.md` / `cmd-search-user.md`）を全部読み、そこに事実として記録されている
TS版の挙動のうち「明らかにバグ・設計ミスであるもの」を洗い出し、Rust版での扱いを決めたもの。

各項目の「現行の挙動」は上記仕様書に事実として記載されているものだけを書いた。
仕様書が「不明」としている点に踏み込む必要があった箇所は **推測** と明記する。

---

## 0. 判定の既定方針

以下を既定の判断基準とする。合理的な理由があれば逸脱してよいが、その場合は該当行の「理由」欄に逸脱の旨を書く。

1. **観測可能なインターフェースは維持する** — コマンド名・サブコマンド名・フラグ名・JSON出力の構造・終了コード。
   利用者のスクリプトが壊れるため。
2. **明確な不具合は修正する** — NaN素通し、レート制限の誤判定、サニタイズ漏れ、トークン漏洩の余地。
3. **ロケール依存・UTF-16依存は Rust の正しい挙動に寄せる** — UTF-8基準、明示的なタイムゾーン指定。
   絵文字を含む入力でパニックしないことを最優先する。
4. **設定ファイルとトークン暗号化のフォーマットは完全互換を維持する** — 既存利用者が再ログインせずに
   移行できることが条件。TS版が書いた `~/.slack-cli/config.json` を読め、Rust版が書いたものを
   TS版も読めること。
5. **コマンド間の非対称は、より正しい方に揃える** — 名前解決あり・ページネーションあり・検証あり。

「分類」欄の語の定義:

- **バグ** — 実装の意図と結果が食い違っている。コード上のコメントやヘルプ文言と挙動が矛盾しているものを含む。
- **仕様** — 意図してそう作られていると読めるもの。移植先でも尊重する対象。
- **曖昧** — ソースからは意図が判定できないもの。判断の根拠を「理由」欄に書く。

---

## 1. 分類サマリ

| カテゴリ | 項目数 | うち修正 | うちそのまま再現 | うち選択制 |
| --- | --- | --- | --- | --- |
| A. 数値・文字列パース | 5 | 5 | 0 | 0 |
| B. レート制限・リトライ・並行 | 5 | 5 | 0 | 0 |
| C. 文字幅・切り詰め（UTF-16依存） | 4 | 4 | 0 | 0 |
| D. 日時・ロケール・タイムゾーン | 6 | 6 | 0 | 0 |
| E. 端末サニタイズ漏れ | 4 | 4 | 0 | 0 |
| F. セキュリティ（トークン・ファイル） | 5 | 3 | 2 | 0 |
| G. コマンド間の非対称 | 22 | 16 | 5 | 1 |
| H. 設定ファイルとトークン暗号化 | 5 | 3 | 2 | 0 |
| J. 基盤・その他 | 10 | 8 | 2 | 0 |
| **合計** | **66** | **54** | **11** | **1** |

---

## 2. A. 数値・文字列パース

| 項目 | 現行の挙動 | 分類 | Rust版の方針 | 理由 | 影響を受けるコマンド |
| --- | --- | --- | --- | --- | --- |
| A1 `parseLimit` の NaN 素通し | `parseInt(limit \|\| String(default), 10)` を NaN チェックなしで返す。`--limit abc` は `NaN` がそのまま Slack API のパラメータに乗る | バグ | **修正**。厳格パース（`u32`）にし、失敗時は `✗ Error: --limit must be a positive integer` で終了コード1 | 数値でない入力を黙って不正値に変換して外部APIへ送るのは診断不能な失敗を生む。Rust の整数型に NaN 相当が存在しないため、忠実再現するなら「limit キー自体を送らない」等の作り込みが要るが、それは元の意図でもない | `channels` / `members` / `bookmark list` / `scheduled list` / `users list` / `unread` |
| A2 `parseInt` の前方一致 | `"12abc"` → 12、`"5abc"` → 5、`"3.7"` → 3、`" 5"` → 5 と、先頭の数字列だけを読んで残りを捨てる | バグ | **修正**。前後の空白 trim のみ許し、それ以外の非数字が混ざればエラー | 入力ミス（`--number 20O` のようなタイプミス）を黙って別の値として受理する。エラーにするほうが利用者にとって安全 | `search --number` / `search --page` / `history --number` および A1 の全 `--limit` |
| A3 `users list --limit abc` が「0件・成功」になる | `parseLimit` が NaN → `listUsers` の `!Number.isFinite(limit) \|\| limit <= 0` 分岐で空配列を返す → `No users found` を stdout に出して**終了コード0** | バグ | **修正**。A1 によりパース時点でエラー終了（コード1） | 「該当ユーザー0件」と「引数が不正」が終了コード0で区別できず、スクリプトが空振りに気づけない。仕様書も「意図的に緩いのか事故なのかソースからは判断できない」としているが、成功扱いにする合理的な理由が見当たらない | `users list` |
| A4 `parseFloat` の前方一致と NaN 素通し | `formatTimestampFixed("abc")` は各 `getUTC*` が NaN を返し `"NaN-NaN-NaN NaN:NaN:NaN"` を出力する。`formatSlackTimestamp` は `"Invalid Date"` になる | バグ | **修正**。Slack ts のパースに失敗したら `(invalid timestamp)` を出す。パニックさせない | 出力に `NaN` が並ぶのは表示として壊れている。Rust では `f64` の前方一致パースを自前実装してまで再現する価値がない | `history` / `search` / `unread` の時刻表示経路 |
| A5 制約チェックの二重防御 | `optionValidators.searchCount` 等が preAction で 1–100 を検証したあと、`parseCount` が同じ範囲で再クランプする。`validators.ts` は `API_LIMITS` 定数を参照せず数値リテラルを直書きしている（値は一致） | 仕様 | **修正（統合）**。範囲は定数1箇所に集約し、検証も1回にする。境界値での結果は変わらない | 同じ制約が2箇所にあり、片方だけ直すと不整合になる。定数と直書きリテラルの二重管理も同じ問題 | `search` / `history` |

---

## 3. B. レート制限・リトライ・並行

| 項目 | 現行の挙動 | 分類 | Rust版の方針 | 理由 | 影響を受けるコマンド |
| --- | --- | --- | --- | --- | --- |
| B1 レート制限の判定がエラーメッセージの部分一致 | `BaseSlackClient.handleRateLimit(error)` は `error.message?.includes('rate limit')` が真のときだけ**固定5秒**待つ。HTTP 429 ステータスも `Retry-After` ヘッダも見ていない | バグ | **修正**。HTTP 429（および 5xx / Slack の `ratelimited`）で判定し、`Retry-After` ヘッダの秒数を起点にする | SDK のエラー文言に依存した判定は SDK 更新で壊れる。Rust では `@slack/web-api` を使わず reqwest 直叩きになるため、そもそもその文言が存在しない。忠実再現は不可能かつ無意味 | `unread`（単一チャンネルの `conversations.history` / `search.messages` の各ページ / `conversations.info`）。他コマンドの経路からは元々呼ばれていない |
| B2 指数バックオフが未実装 | `RATE_LIMIT.RETRY_CONFIG = { retries: 3, factor: 2, minTimeout: 1000, maxTimeout: 30000 }` が定義されているが、参照されているのは `retries` のみ。`factor` / `minTimeout` / `maxTimeout` / `BATCH_SIZE` / `BATCH_DELAY_MS` は全ファイルを通して未参照。実待機は固定5秒 | バグ | **修正**。`Retry-After` × 2^attempt（上限60秒）に ±20% のジッタを乗せる。最大リトライ回数は現行と同じ3回（初回含め計4回） | 定数が意図を示しているのに実装が追いついていない。固定5秒は Slack の Tier 別レート制限に対して短すぎたり長すぎたりする。上限値と回数は現行の観測可能な範囲（最大待ち時間）から大きく逸脱しない | 全コマンド |
| B3 429 で即失敗する経路が大半 | `WebClient` を `retryConfig: { retries: 0 }` で生成し、「レート制限を手動で扱うため」とコメントがある。しかし B1 の手動処理を通るのは unread 系だけで、他は 429 が即例外になる | バグ | **修正**。B1/B2 のリトライをクライアント層に一本化し、全メソッドに効かせる | コメントの意図と実装が食い違っている典型。書き込み系（`chat.postMessage` 等）のリトライは二重投稿を懸念しうるが、429 は「リクエストが処理されなかった」ことを意味するので副作用は発生していない | `channels` / `members` / `send` / `send-ephemeral` / `edit` / `delete` / `upload` / `download` / `search` / `users` / `usergroups` / `reaction` / `pin` / `bookmark` / `canvas` / `scheduled` / `reminder` / `draft send` / `history` |
| B4 並列度リミッタが実質効いていない | `pLimit(RATE_LIMIT.CONCURRENT_REQUESTS = 3)` を全 Operations で共有しているが、実際に通しているのは `getUserInfo`（`users.info`）だけ。`conversations.list` / `chat.*` / `users.list` / `search.messages` などは素通し | バグ | **修正**。全 API 呼び出しを共有セマフォ（`tokio::sync::Semaphore`、許可数3）経由にする | リミッタを持ちながら効いていない。現行はほぼ直列実行なので、全通しにしても遅くならない。`members` / `usergroups members` の並列 `users.info` は入力順を保つため `buffered(3)`（`buffer_unordered` ではない）を使う | 全コマンド |
| B5 未読スキャンの並列度が合算される | `listUnreadChannels` と `enrichUnreadChannels` がそれぞれ `pLimit(15)` を**その場で新規生成**する。共有リミッタ（3）とは独立なので、未読スキャン中は最大 15+3 の並列になりうる | 曖昧 | **修正**。未読スキャン専用の許可数15を共有セマフォの上限として扱い、合算されないようにする | 「未読スキャンだけ並列度を上げる」意図自体は尊重する（定数名 `UNREAD_SCAN_CONCURRENT_REQUESTS` が意図を示している）。ただし別インスタンスを都度作って合算されるのは意図の表れというより実装の副産物と読める | `unread`（全チャンネルモード） |

---

## 4. C. 文字幅・切り詰め（UTF-16依存）

| 項目 | 現行の挙動 | 分類 | Rust版の方針 | 理由 | 影響を受けるコマンド |
| --- | --- | --- | --- | --- | --- |
| C1 `padEnd` が UTF-16 コードユニット基準 | 日本語チャンネル名・絵文字を含む値で列が崩れる。`padEnd(17)` は日本語17文字を17カラムと数えるが、端末上は34カラム占める | バグ | **修正**。`unicode-width` による**表示幅**でパディングする | 「表を揃える」という実装意図に対して結果が揃っていない。Rust の `format!("{:<17}")` は Unicode スカラー値数なので TS とも表示幅とも一致せず、どちらにせよ現行とは変わる。ならば意図どおりに揃うほうを選ぶ | `channels --format table` / `members --format table` / `usergroups members --format table` / `reminder list --format table` / `bookmark list --format table` / `unread --format table` |
| C2 `substring` / `slice` が UTF-16 基準でサロゲートペアを分断しうる | `channels` の purpose は31文字以上で `substring(0, 27) + '...'`、`bookmark list` の Text は `slice(0, 38)`、`reminder list` の text は `slice(0, 28)`、`draft list` の message は60文字超で先頭60文字 + `...` | バグ | **修正**。切り詰めも `unicode-width` の表示幅基準にし、grapheme cluster を割らない | Rust の `&s[..27]` は UTF-8 バイト境界外でパニックする。文字列を壊さないことが最優先。C1 と基準を揃えないと結局列が崩れる | `channels` / `bookmark list` / `reminder list` / `draft list` |
| C3 `console.table` 依存 | `draft list` / `scheduled list` / `users list` / `users presence` / `pin list` の table 出力は Node の `console.table` に丸投げ。罫線・`(index)` 列・文字列のシングルクォート囲み・幅計算がすべて Node ランタイム依存 | バグ | **修正**。他コマンドと同じ手書き固定幅（ヘッダ + `─` 罫線）に統一する | Rust に `console.table` の相当物は無く、1:1 再現には Node の内部仕様を固める作業が要る。かつ `(index)` 列やクォートは CLI の表示として不要。`--format table` は人間向けの表示であり、機械可読の役割は `--format json` が担っているため互換破壊の影響は限定的 | `draft list` / `scheduled list` / `users list` / `users presence` / `pin list` |
| C4 `maskToken` の `substring` が UTF-16 基準 | 長さ9以下なら `****`、それ以外は `先頭4文字 + "-****-****-" + 末尾4文字`。閾値の比較は `<=` | バグ（軽微） | **修正**。`chars()` ベースで実装し、閾値と出力形式は現行のまま維持 | Slack トークンは ASCII なので実害は無いが、平文トークンとして非 ASCII が保存されていた場合にバイト境界でパニックする。閾値を1ずらすと既存テストが落ちるので境界は動かさない | `config get` / `config profiles` |

---

## 5. D. 日時・ロケール・タイムゾーン

| 項目 | 現行の挙動 | 分類 | Rust版の方針 | 理由 | 影響を受けるコマンド |
| --- | --- | --- | --- | --- | --- |
| D1 `toLocaleString()` による出力 | `formatSlackTimestamp` = `new Date(ts*1000).toLocaleString()`。実行環境のロケールと TZ に依存し、日本語環境では `2026/8/17 1:23:45` になる | バグ | **修正**。`YYYY-MM-DD HH:MM:SS`（UTC）に統一する。`history` の `formatTimestampFixed` と同じ形式 | ロケール依存出力は Rust の標準機能では再現できず、`icu` 系クレートを入れても環境ごとに変わる値は CLI 出力として不適切。同一 CLI 内で `history` が UTC 固定、`unread` がロケール依存という食い違いも解消される | `unread`（単一チャンネル・全チャンネル両モードの table / simple / json） |
| D2 `toLocaleDateString()` による出力 | `channel info --format table` の `Created:` が `new Date(created*1000).toLocaleDateString()`。日本ロケールで `2019/4/1`、en-US で `4/1/2019` | バグ | **修正**。`YYYY-MM-DD`（UTC）にする | D1 と同じ。`channels --format table` の Created 列が既に `YYYY-MM-DD`（UTC）なので、そちらに揃う | `channel info` |
| D3 `Date.parse` の実装依存パース | `--since` / `--at` は `Date.parse` に渡され、ISO 8601 のほか V8 が受理する緩い形式（`"Jan 1, 2024"` / `"2024/01/01"` / `"Mon Jan 01 2024 10:00:00 GMT+0900"` 等）も通る | バグ | **修正**。受理する形式を明示列挙する: RFC3339 / ISO 8601（`Z`・オフセット付き）、`YYYY-MM-DDTHH:MM[:SS]`、`YYYY-MM-DD HH:MM[:SS]`、`YYYY-MM-DD`、および全桁数字の Unix秒 | V8 のフォールバックパーサは仕様化されておらず Rust で完全再現できない。受理範囲を明示して、外れたら `Invalid schedule time format...` の既存文言でエラーにする（文言は現行のまま維持） | `send --at` / `history --since` / `reminder add --at` |
| D4 `Date.parse` の TZ 解釈が日付と日時で非対称 | ECMAScript の規則により、`"2024-01-01"`（日付のみ）は **UTC**、`"2024-01-01T00:00:00"`（TZ 指定なしの日時）は **ローカル時刻**として解釈される | バグ | **修正**。TZ 指定が無い入力はすべて**ローカルタイムゾーン**として解釈する | 同じ「TZ を書かなかった」入力が形式によって別の意味になるのは利用者が予期しない。`--since "2026-01-01 10:00:00"` と打つ利用者はローカル時刻を意図しており、そちらに統一する。**入力はローカル解釈・出力は UTC 表示**という規則になる点をヘルプと本書に明記する | `send --at` / `history --since` / `reminder add --at` |
| D5 `channels --format json` の `created` が偽の時刻を含む | Unix秒 → UTC 日付文字列 `YYYY-MM-DD` に落として時分秒を捨て、そこに文字列連結で `T00:00:00Z` を付け直している | バグ | **修正**。実際の作成時刻を RFC3339（UTC、ミリ秒付き）で出す | 情報を捨てたうえで存在しない時刻（常に 00:00:00Z）を付けるのは、JSON を機械処理する側から見て誤ったデータ。JSON のキー名と型（文字列）は変わらない | `channels --format json` |
| D6 `formatUnixTimestamp` が例外を投げうる | `timestamp * 1000` が `Date` の有効範囲（絶対値 8.64e15 ms）を超えると `toISOString()` が `RangeError` を投げる | バグ | **修正**。範囲外は `(invalid timestamp)` を出す。パニック・例外にしない | 表示のための整形関数がプロセスを落とすのは不適切。A4 と同じ扱いに揃える | `channels` / `channel info` |

---

## 6. E. 端末サニタイズ漏れ

前提: TS版は原則としてすべての出力値に `sanitizeTerminalText`（OSC/ANSI シーケンス除去 + C0/DEL/C1 制御文字除去、
TAB と LF のみ残す）を通している。以下はその原則から漏れている箇所であり、
**利用者が渡した引数がそのまま端末に流れる**ため、エスケープシーケンス注入の余地がある。

| 項目 | 現行の挙動 | 分類 | Rust版の方針 | 理由 | 影響を受けるコマンド |
| --- | --- | --- | --- | --- | --- |
| E1 チャンネル操作の成功メッセージが未サニタイズ | `✓ Joined channel #<-c の生値>` / `✓ Left channel #...` / `✓ Invited user(s) to channel #...` / `✓ Topic updated for #...` / `✓ Purpose updated for #...` の `#` 以降は `-c` に渡された文字列そのまま。仕様書に「他の出力箇所は全部サニタイズしているのに、ここだけ漏れている」と明記 | バグ | **修正**。`sanitizeTerminalText` を適用する | サニタイズ漏れは設計意図の欠落であって仕様ではない。出力される文字列は、エスケープを含まない通常のチャンネル名では変わらない | `join` / `leave` / `invite` / `channel set-topic` / `channel set-purpose` |
| E2 リアクション・ピンの成功メッセージが未サニタイズ | `✓ Reaction <--emoji の生値> added to message in #<--channel の生値>` / `✓ Pin added to message in #...`。仕様書に「サニタイズを通していない」と明記 | バグ | **修正**。同上 | 同上。`--emoji` は利用者入力なので `--channel` と同じリスクがある | `reaction add` / `reaction remove` / `pin add` / `pin remove` |
| E3 ブックマーク（stars）の成功メッセージが未サニタイズ | `✓ Saved message <--ts の生値> in <--channel の生値>` / `✓ Removed saved item ...`。仕様書に「出力値は API レスポンスではなく入力値をそのまま（サニタイズもされていない）」と明記 | バグ | **修正**。同上 | 同上 | `bookmark add` / `bookmark remove` |
| E4 `mapChannelToInfo` の `id` が未サニタイズ | `name` / `purpose` は `sanitizeTerminalText` を通すが `id` はそのまま | バグ（軽微） | **修正**。同上 | チャンネル ID は Slack API 由来で実害はまず無いが、サニタイズを「出力の直前に一律で掛ける」設計にすれば個別の漏れが構造的に起きなくなる | `channels` |

**構造上の対策**: Rust版では「出力ヘルパを通らない `println!` を書けない」構造にする。
成功メッセージ・表・エラーのすべてを 1 つの出力モジュールに集約し、
そのモジュール内でサニタイズを掛ける。E1〜E4 のような個別の漏れは、
個々の呼び出し側の注意ではなく型で防ぐ。

なお `wrapCommand` のエラー出力の処理順（`extractErrorMessage` → `sanitizeTerminalText` → `redactSlackTokens`）は
**そのまま維持する**。コード内コメントに「エスケープシーケンスで分断されたトークンも確実に伏せるため
先にサニタイズする」と明記されており、順序を逆にすると伏字が漏れる。これは正しい設計。

---

## 7. F. セキュリティ（トークン・ファイル）

| 項目 | 現行の挙動 | 分類 | Rust版の方針 | 理由 | 影響を受けるコマンド |
| --- | --- | --- | --- | --- | --- |
| F1 `download --url` がホスト無検証で Bearer を付与 | `--url` に渡された URL をホスト検証せず、無条件に `Authorization: Bearer <token>` を付けて GET する。Slack 以外のドメインを渡せばトークンがそのホストに送られる | バグ | **修正**。Slack のファイル配信ドメイン（`*.slack.com` / `files.slack.com` / `slack-files.com` / `*.slack-edge.com`）に対してのみ Bearer を付ける。それ以外のホストへは**トークンを付けずに** GET する | トークン漏洩の余地は互換性より優先する。エラーにせず「トークンを外す」ことで、Slack 外の公開 URL を渡す既存の使い方（あれば）は動き続ける。仕様書も「互換性ではなくセキュリティの判断として扱うべき箇所」としている | `download --url` |
| F2 リダイレクト追従時の Authorization 保持 | TS版はダウンロードに Node の `fetch` を使っており、既定でリダイレクトを追従する。追従先へ認証ヘッダが引き継がれるかは Node の実装依存で、仕様書には記載がない（**推測**: 同一オリジンなら引き継がれ、クロスオリジンでは落ちる） | バグ | **修正**。Web API 呼び出しはリダイレクト禁止（`redirect::Policy::none()`）。ファイルダウンロードのみ追従を許すが、**ホストが変わった時点で `Authorization` を落とす** | F1 と同じ理由。API 呼び出しでリダイレクトを追う正当な理由はなく、notion-cli が既にこの方針を採っていて流用できる | `download` / 全 API 呼び出し |
| F3 レガシー鍵が固定パスフレーズ由来 | v1（AES-256-CBC）の鍵は `PBKDF2("slack-cli-key", "slack-cli-salt-v1", 100000, 32)`。ソースを見れば誰でも復号できる | 仕様 | **そのまま再現（復号のみ）**。新規暗号化には決して使わない | 既存トークンの読み出し互換のためだけに存在する、と仕様書に明記されている。既定方針4（再ログイン不要）の要件そのもの。復号したトークンは次回の書き込み時に v2 形式へ移行する（H1 参照） | `config get` / `config set` / 全コマンドの認証経路 |
| F4 `download` が既存ファイルを無警告で上書き | `createWriteStream` + `pipeline` で保存し、既存ファイルの確認をしない | 仕様 | **そのまま再現** | `--output` を明示している以上、上書きは利用者の意図と読める。`--force` 相当のフラグ追加は観測可能インターフェースの追加になるため今回は行わない。`--output` 未指定時にカレントディレクトリへ書く点はリリースノートで注意喚起する | `download` |
| F5 設定・ドラフトの read-modify-write に排他が無い | 書き込み自体は「一意な temp 名 + `wx` + `rename`」で原子的だが、読み込み〜書き込みの間にロックが無い。同時に別プロファイルを `config set` すると後勝ちで片方が失われる（TOCTOU）。`drafts.json` も同じ | バグ | **そのまま再現**（今回は修正しない） | 修正するにはファイルロック（advisory lock）の導入が必要で、ロックファイルの残骸処理・非 Unix 環境の扱いなど、移植の範囲を超えた設計が要る。CLI の実行が単発である以上、実際に踏む確率は低い。リリースノートで既知の制限として明記し、別途対応する | `config set` / `config use` / `config clear` / `draft save` / `draft delete` / `draft send` |

---

## 8. G. コマンド間の非対称

| 項目 | 現行の挙動 | 分類 | Rust版の方針 | 理由 | 影響を受けるコマンド |
| --- | --- | --- | --- | --- | --- |
| G1 チャンネル名→ID 解決の有無 | **解決する**: `channel info` / `set-topic` / `set-purpose` / `join` / `leave` / `invite` / `members` / `edit` / `delete` / `upload` / `canvas list` / `reaction` / `pin` / `scheduled list` / `scheduled cancel`。**解決しない**（生値を Slack API に渡す）: `send` / `send-ephemeral` / `draft send` / `bookmark add` / `bookmark remove` | バグ | **修正（フォールバック方式）**。まず生値のまま送り、Slack が `channel_not_found` を返したときだけ名前解決して1回だけ再試行する | 既定方針5（解決ありに揃える）に沿うが、無条件に先へ解決を挟むと `channels:read` スコープの無いトークンで `send` が動かなくなる回帰が起きる（現行は `chat.postMessage` が `#name` を受け付けるため動いている）。**既定方針からの逸脱**であり、逸脱の理由は「解決を必須にするとスコープ要件が増えて既存利用者が壊れるため」。フォールバック方式なら、いま動いている呼び出しは全部そのまま動き、いままでエラーだった呼び出しだけが救われる | `send` / `send-ephemeral` / `draft send` / `bookmark add` / `bookmark remove` |
| G2 `--format` 検証の有無 | **検証あり**（未知値はエラー）: `channel info` / `members` / `history` / `search` / `users *` / `usergroups *` / `pin list` / `upload` / `canvas *` / `bookmark list` / `draft list` / `scheduled list` / `reminder list`。**検証なし**（未知値は table にフォールバック）: `channels` / `unread` / `download` | バグ | **修正**。全コマンドで検証する。`--format xml` は `✗ Error: Invalid format 'xml'. Must be one of: table, simple, json`（文言は現行のまま） | 既定方針5そのもの。検証漏れの3コマンドだけ「タイポが黙って別形式で成功する」のは事故のもと | `channels` / `unread` / `download` |
| G3 `--limit` 検証の有無 | **どのコマンドでも検証されていない**。`scheduled list` は preAction が `format` のみ、`users list` / `channels` / `members` / `bookmark list` / `unread` も同様 | バグ | **修正**。A1/A2 により全コマンドで厳格パース + 範囲検証 | 同上 | `channels` / `members` / `bookmark list` / `scheduled list` / `users list` / `unread` |
| G4 `simple` が table にフォールバックする | `renderByFormat` は `simple` レンダラが未定義のとき table に落ちる。該当するのは `upload` / `users info` / `users lookup`。仕様書に「バグに見えるが現行仕様」と記載 | バグ | **修正**。3コマンドに simple レンダラを実装する。`upload` は `permalink` を1行、`users info` / `users lookup` は `<id>\t<name>\t<real_name>`（`users list --format simple` と同じ列構成） | ヘルプが `table\|simple\|json` を謳っているのに片方が別形式になるのは、仕様というより実装漏れ。他コマンドの simple の作り（TSV か単一値1行）に合わせる | `upload` / `users info` / `users lookup` |
| G5 ページネーションの有無 | **全ページ取得**: `conversations.list` / `users.conversations` / `conversations.replies` / `users.list`。**1ページのみ**: `conversations.members`（`members`）/ `stars.list`（`bookmark list`）/ `files.list`（`canvas list`）/ `chat.scheduledMessages.list`（`scheduled list`）/ `conversations.history`（`history`） | バグ | **修正**（`history` を除く）。`--limit` で指定された件数に達するまでカーソルを追う。`getChannelMembers` の `nextCursor` は既に返り値にあるのに呼び出し側が捨てている | 既定方針5（ページネーションありに揃える）。`history` だけは例外で、`--number` の上限1000が `conversations.history` の `limit` 上限と一致し1リクエストで足りるため現状維持とする | `members` / `bookmark list` / `canvas list` / `scheduled list` |
| G6 `channels --limit` が総件数の上限にならない | `--limit` は `conversations.list` のページサイズとして渡るだけで、do-while が `next_cursor` を追い続けるため全件が返る。`--limit 100` でも 500 チャンネルあれば 500 件表示される | バグ | **修正（既定値のみ据え置き）**。`--limit` を「取得・表示する総件数の上限」の意味に統一する。ただし `channels` の**既定値だけは「無制限」に変更**し、明示指定時のみ効かせる | フラグ名と挙動が矛盾している。ただし既定値100をそのまま総件数上限にすると、いままで全件出ていた出力が100件に切られてスクリプトが壊れる。既定を無制限にすれば「引数なしの実行結果は不変」かつ「`--limit` が意味を持つ」を両立できる。ヘルプ上の既定値表記は `unlimited` に変わる | `channels` |
| G7 チャンネルID判定の正規表現が2種類 | `ChannelResolver.isChannelId` は `/^[CDG][A-Z0-9]{8,}$/`、`formatValidators.channelId` は `/^[CDG][A-Z0-9]{10,}$/`。実際に使われるのは前者のみ | バグ | **修正**。`{8,}` に統一し、`formatValidators.channelId` は削除する | 緩い方に揃える。`{10,}` に寄せると、いま解決できていた9〜10文字の ID が名前扱いになって解決に失敗する回帰が起きる。後者はどこからも使われていないデッドコード | 全チャンネル系コマンド |
| G8 `--format` 許容値の集合が2種類 | `formatValidators.outputFormat` は `table` / `simple` / `json` / `compact` の4値、`optionValidators.format` は3値。実際に使われるのは後者のみで、`compact` を出力できるコマンドは存在しない | バグ | **修正**。`compact` を削除し、`OutputFormat` enum を `Table` / `Simple` / `Json` の3値にする | 実装されていない値を受理する定義はデッドコード。`compact` を実装する要件も見当たらない | なし（現状 `compact` は到達不能） |
| G9 reaction / pin のタイムスタンプ検証で "thread" 文言が出る | `optionValidators.reactionTimestamp` / `pinTimestamp` は `formatValidators.threadTimestamp` を流用しており、エラー文言が `Invalid thread timestamp format` になる。`--timestamp` はスレッドではなくメッセージの ts | バグ | **修正**。`Invalid message timestamp format`（`edit` / `delete` と同じ文言）にする | 定数の使い回しによる文言のずれ。利用者にとって誤解を招く。エラーメッセージ本文は終了コードと違ってスクリプトが依存しにくく、破壊の影響は小さい | `reaction add` / `reaction remove` / `pin add` / `pin remove` |
| G10 バリデーションエラーの `Error:` 二重前置 | `createValidationHook` が `thisCommand.error('Error: ' + msg)` を呼び、commander がさらに `error: ` を前置するため、実出力は `error: Error: Invalid format 'xml'. ...` になる | バグ | **修正**。バリデーション由来のエラーも `wrapCommand` と同じ `✗ Error: <msg>` 形式（stderr、終了コード1）に統一する | 仕様書も「バグに見えるが現行挙動」としている表示の誤り。clap は commander と前置文言が違うため、どのみち完全互換は不可能。ならば CLI 内で1つの形式に揃える | preAction バリデータを持つ全コマンド |
| G11 必須性の担保が2経路で文言が違う | commander の `requiredOption`（`error: required option '-c, --channel <channel>' not specified`）と、手書きバリデータ（`error: Error: --channel is required`）が混在。`send-ephemeral` の `--channel` / `--user` / `--message` と `draft save` の `--message` が後者 | バグ | **修正**。すべて clap の `required = true` に寄せ、G10 の統一形式で出す | 同じ「必須オプション欠落」が2種類の文言で出るのは一貫性の欠如。clap のエラーフォーマットは commander と一致しないので、文言互換のために2経路を維持する意味がない | `send-ephemeral` / `draft save` / その他 requiredOption を使う全コマンド |
| G12 相互排他チェックの経路が2種類 | preAction バリデータ経由（`error: Error: ...`）と、action 内の素の `throw`（`✗ Error: ...`）が混在。後者は `users presence` の `--id`/`--name`、`usergroups members` の `--id`/`--handle`、`draft save` の `--channel`/`--user` | バグ | **修正**。G10 の統一形式に揃える。判定ロジックは手書きのまま維持し、clap の `conflicts_with` には置き換えない | 表示の統一。ただし判定は手書きのまま残す。clap の組み込み排他に置き換えると評価順が変わり、複数違反時に出るメッセージが変わってしまう（TS版は配列順に評価して最初の1件で終了する） | `users presence` / `usergroups members` / `draft save` |
| G13 不明ユーザーの表示規則が2系統 | `history`: `message.user` があって users マップに無ければ `Unknown User`、`bot_id` があれば `Bot`、どちらも無ければ `Unknown`。`unread`: users マップに無ければ**ユーザーIDそのまま**、`message.user` が無ければ `unknown`（小文字） | バグ | **修正**。`history` 側の規則（`Unknown User` / `Bot` / `Unknown`）に統一する。table / simple / json のすべての経路に適用 | 同じ概念に2系統の規則があり、`unknown` と `Unknown` の大小まで違う。`unread --format json` の `author` の値が変わるため破壊的だが、値の変更であってキー構造は変わらない | `unread`（table / simple / json） |
| G14 0件時に `--format` を無視して人間向けテキストを出す | `canvas read` → `No sections found in canvas`、`canvas list` → `No canvases found in channel`、`bookmark list` → `No saved items found`、`channels` → `No channels found`、`members` → `No members found`、`users list` → `No users found`、`usergroups list` → `No usergroups found`、`pin list` → `No pinned items found`、`search` → `No messages found`。いずれも `--format json` 指定時も同じテキストが出る。一方 `history --format json` は `{"channel":..., "messages": [], "total": 0}` を出す | バグ | **修正**。`--format json` のときは空配列 `[]` または空構造の JSON を出す。table / simple のテキストは現行のまま維持 | JSON をパースする側から見て現状は壊れた出力（`No canvases found in channel` は JSON ではない）。`history` が既に正しい側の挙動をしており、そちらに揃える。既定方針1（JSON構造の維持）は「壊れた出力を維持する」ことまでは要求しない | `canvas read` / `canvas list` / `bookmark list` / `channels` / `members` / `users list` / `usergroups list` / `pin list` / `search` |
| G15 `--count-only` が `--format` を上書きする | `unread` の全チャンネルモードでは `--count-only` が `count` という第4のフォーマッタを選ぶため、`--format json --count-only` でも JSON ではなくテキストが出る。単一チャンネルモードでは逆に、`--count-only` はフォーマッタ内のフラグとして扱われ JSON のまま | バグ | **修正**。`--count-only` は「メッセージ本体を出さない」フラグとして扱い、`--format` の選択は常に尊重する。`--format json --count-only` は件数だけを含む JSON を出す | `--count-only` は出力の**内容**を絞るフラグであって出力**形式**の指定ではない。同じフラグがモードによって別の意味になるのは明確な不整合 | `unread` |
| G16 成功メッセージの `#` 付与が二重・不適切になる | `#` はコード側で常に前置されるため、`send -c "#general"` は `✓ Message sent successfully to ##general`、`edit -c C0123456789` は `✓ Message updated successfully in #C0123456789` になる | バグ | **修正**。`formatChannelName` と同じ規則（先頭が既に `#` なら足さない）を全成功メッセージに適用し、チャンネル ID を渡されたときは `#` を付けない | `channel-formatter.ts` に「先頭が `#` でなければ付ける」という正しいヘルパが既にあるのに、成功メッセージ側で使われていない。ID に `#` を付けるのは端的に誤り | `send` / `send-ephemeral` / `edit` / `delete` / `upload` / `join` / `leave` / `invite` / `channel set-topic` / `channel set-purpose` / `reaction` / `pin` / `draft send` |
| G17 `--emoji :tada:` でコロンが二重になる | API に渡す `name` は先頭・末尾の `:` を1個ずつ除去した値だが、成功メッセージには `--emoji` の生値をそのまま埋めるため `✓ Reaction ::tada:: added to ...` になる | バグ | **修正**。正規化後の名前を `:name:` の形で表示する | API に渡す値は正規化しているのに表示だけ生値、という単純な不整合 | `reaction add` / `reaction remove` |
| G18 `invite` がユーザー名を解決しない | `--users` はカンマ分割・trim・空要素除去のみで、形式チェックも名前解決もしない。`@daichi` を渡すとそのまま `conversations.invite` に送られて Slack 側でエラーになる。`resolveUserIdByName` は `send --user` / `users presence --name` / `draft send` で使われているのに `invite` からは呼ばれない | バグ | **修正**。`U` で始まる ID 形式の要素はそのまま使い、それ以外は `resolveUserIdByName` で解決する | 既定方針5（名前解決ありに揃える）。現行でエラーになっていた入力が成功するようになるだけで、いま動いている呼び出しは壊れない | `invite` |
| G19 API の部分失敗を見ない | `invite --force` 時にレスポンスの `errors` フィールドを見ないため、一部ユーザーの招待に失敗しても出力から判別できない。`join` は Slack が返す `already_in_channel` warning を見ないため成功扱いになる | バグ | **修正**。部分失敗・warning を stderr に警告として出す。stdout の成功メッセージと終了コードは変えない | 利用者が結果を知る手段が無いのは情報の欠落。stdout と終了コードを変えなければスクリプトは壊れない | `invite` / `join` |
| G20 JSON の「値なし」の扱いがコマンドで違う | `channel info --format json` は `num_members` が undefined のときキーごと消える。`members --format json` は `name` / `real_name` が undefined のとき `null` ではなく空文字 `""` を入れる | 仕様 | **そのまま再現**。`channel info` は `skip_serializing_if = "Option::is_none"`、`members` は空文字を入れる | **既定方針5からの逸脱**。逸脱の理由は既定方針1（JSON 出力の構造は観測可能インターフェース）を優先するため。キーの有無や型が変わると `jq` によるパイプが壊れる。表示の非対称と違い、ここは機械が読む契約 | `channel info` / `members` |
| G21 JSON の `text` だけメンションが未置換 | `history` / `unread` の JSON 出力は `<@U0999ZZZZ>` のまま。table / simple は `@alice` に置換される | 仕様 | **そのまま再現** | JSON は生データを渡す層、メンション置換は表示層の責務、という解釈が成り立つ。`history --format json` には既に `user_id` フィールドがあり、利用者側で置換できる。既定方針1に従う | `history` / `unread` |
| G22 `unread` の `--limit` / `--mark-read` の適用範囲が非対称 | `--limit` は全チャンネルモードの表示件数にしか効かず、単一チャンネルのプレビューは定数50固定。`--mark-read` は表示件数と無関係に**全未読チャンネル**を既読化する | 仕様 | **そのまま再現**。ヘルプに適用範囲を明記する | `--mark-read` は「未読を既読にする」意図であって表示の絞り込みとは独立、と読める。表示件数に連動して既読化の範囲が変わるほうがむしろ驚きが大きい。単一チャンネルの50件固定は `DEFAULTS.UNREAD_MESSAGE_PREVIEW_LIMIT` として定数化されており意図的 | `unread` |

---

## 9. H. 設定ファイルとトークン暗号化

**大前提**: このカテゴリは既定方針4により、ファイルフォーマットの完全互換が絶対条件。
以下の「修正」はすべて**フォーマットを変えない範囲**での挙動修正である。

| 項目 | 現行の挙動 | 分類 | Rust版の方針 | 理由 | 影響を受けるコマンド |
| --- | --- | --- | --- | --- | --- |
| H1 `config get`（読み取り）が再暗号化して書き戻す | `ProfileConfigManager.getConfig()` は `isCurrentFormat(token)` が偽（平文またはレガシー v1 形式）のとき、復号結果を v2 で暗号化し直して `saveConfigStore()` する。読み取り操作がディスク書き込みを起こす | バグ | **修正**。読み取り経路（`config get` / `config profiles` / `config current` および全コマンドの認証経路）は復号するだけにし、書き戻さない。v2 への移行は書き込み系（`config set` / `config use` / `config clear`）が走ったときに行う | 読み取り専用のはずの操作に書き込み副作用があると、ファイル権限が無い環境で読み取りすら失敗しうる。また F5（排他なし）と組み合わさると、単に `config get` しただけで並行実行中の他プロセスの書き込みを潰す。移行のタイミングを書き込み系に限定してもフォーマット互換は保たれる（v1・平文はいつまでも読める） | `config get` / `config profiles` / `config current` / 全コマンド |
| H2 `config profiles` が暗号文をマスクして表示する | `listProfiles()` が復号しないため、`maskToken` に暗号化文字列が渡り `v2-****-****-3f2a` のような表示になる。`config get`（復号後をマスク）とは形が違う | バグ | **修正**。復号してからマスクし、`config get` と同じ `xoxb-****-****-1a2b` 形式にする。個別のプロファイルで復号に失敗した場合はその行だけ `<decrypt failed>` を出して処理を続行する | マスクの目的は「どのトークンか識別できる程度に見せる」ことで、暗号文の末尾4文字にはその情報が無い。完全に無意味な表示になっている。復号失敗で `config profiles` 全体が落ちると、壊れた1プロファイルのせいで他が見えなくなるため個別にフォールバックする | `config profiles` |
| H3 `config clear` が存在しないプロファイルでも成功する | `delete store.profiles[name]` が no-op になるだけで、`✓ Profile "xxx" cleared successfully` が出て終了コード0 | バグ | **修正**。存在しないプロファイル名は `✗ Error: Profile "xxx" not found`（既存の `ERROR_MESSAGES.PROFILE_NOT_FOUND` 文言）で終了コード1にする | `config use` は存在しないプロファイルをエラーにするのに `clear` はしない、という非対称。既定方針5（検証ありに揃える）。**ただし冪等な削除を期待するスクリプトは壊れる**ため、判断が割れる論点として §11 に挙げる | `config clear` |
| H4 マスターキー解決の非対称 | 注入キー・環境変数 `SLACK_CLI_MASTER_KEY` は PBKDF2-HMAC-SHA256（salt `slack-cli-master-key-salt-v2`、100000回、32バイト）に通すが、鍵ファイル `~/.slack-cli-secrets/master.key` の内容は**hex をそのまま32バイト鍵として使う**（PBKDF2 を通さない） | 仕様 | **そのまま再現**。分岐を厳密に保つ | 既定方針4の中核。混同すると既存トークンが一切復号できなくなり、全利用者が再ログインを強いられる。鍵ファイルは既に32バイトの CSPRNG 乱数なので PBKDF2 を通す必要が無い、という設計として合理的でもある | 全コマンド |
| H5 `defaultProfile` 再選出が JSON のキー順に依存 | `clearConfig` で既定プロファイルを消したとき、残りの**キー順の先頭**が新しい `defaultProfile` になる。JS のオブジェクトキーは挿入順 | 仕様 | **そのまま再現**。`serde_json` の `preserve_order` フィーチャ（`IndexMap`）を有効にし、パース時のキー順を保つ | `HashMap` にすると選ばれるプロファイルが非決定になり、同じ操作で違う結果が出る。順序保持マップの導入は互換維持のためのコストとして妥当 | `config clear` / `config profiles` |

**そのまま維持するフォーマット仕様**（変更禁止のチェックリスト）:

- 設定ファイル `~/.slack-cli/config.json`、`{ profiles: { <name>: { token, updatedAt } }, defaultProfile }`、2スペースインデント
- 旧フォーマット（トップレベル `token` / `profiles` なし）からの自動移行
- v2 暗号文 `v2:<iv 24hex>:<ct 偶数長hex・空可>:<tag 32hex>`（AES-256-GCM、AAD なし、IV 12バイト）
- v1 暗号文 `<iv 32hex>:<ct 非空・偶数長hex>`（AES-256-CBC、PKCS#7、**復号のみ**）
- 形式判定の条件（セグメント数・hex 判定・長さ・偶数長）を緩めない
- 鍵ファイル `~/.slack-cli-secrets/master.key`（hex64 + 改行）、旧配置 `~/.slack-cli/master.key` からの移行、レガシーファイルは削除しない
- ディレクトリ `0o700` / ファイル `0o600`、temp（`create_new` + `mode(0o600)`）→ `rename` の原子的書き込み
- プロファイル解決順序: 引数 → `store.defaultProfile` → `"default"`
- `~/.slack-cli/drafts.json` の `Draft` 配列フォーマットと、`id`/`message` が文字列の要素だけ残す寛容な読み込み

---

## 10. J. 基盤・その他

| 項目 | 現行の挙動 | 分類 | Rust版の方針 | 理由 | 影響を受けるコマンド |
| --- | --- | --- | --- | --- | --- |
| J1 Slack は HTTP 200 + `ok: false` でエラーを返す | `@slack/web-api` がこれを例外に変換しているため、TS 側のコードには判定が無い。エラーの `data.error` / `data.needed` は SDK が生やすプロパティ | 仕様 | **修正（実装必須）**。レスポンスボディの `ok` が偽ならエラー型に変換し、`error` / `needed` フィールドを保持する。notion-cli 流用の `request_with_retry` は「非2xxのときだけエラー」なので、成功パスに `ok` 判定を足す | SDK が無い Rust では自前実装が必須。`extractErrorMessage` の `missing_scope (needed: a, b)` 加工と、`conversations.list` のスコープ不足フォールバックが両方この情報に依存している | 全コマンド |
| J2 ページネーションに上限が無い | `conversations.list` / `users.conversations` / `conversations.replies` / `users.list` はいずれも `next_cursor` が truthy な限りループし、上限ページ数も同一カーソル検出も無い | バグ | **修正**。notion-cli の安全策を流用する: `MAX_PAGES = 10_000` の上限、前回と同じカーソルが返ったらエラー、`has_more` 相当と `next_cursor` の不整合を検出 | サーバ側の不具合で無限ループするとプロセスが止まらない。上限に達するのは正常系では起こりえない値なので、既存の挙動は変わらない | `channels` / `unread` / `history --thread` / `send --user` / `users list` / `users presence --name` / 全チャンネル名解決 |
| J3 終了コードが 0 / 1 のみ | バリデーションエラー・API エラー・設定エラー・commander のエラーがすべて 1。`SlackCliError` の `code`（`VALIDATION_ERROR` 等）は文字列プロパティとして持つだけで終了コードに反映されない | 仕様 | **そのまま再現**。成功0 / 失敗1 の2値を維持する | 既定方針1（終了コードは観測可能インターフェース）。notion-cli は種別ごとに 3 / 4 などを返すが、それを持ち込むと既存スクリプトの `if [ $? -eq 1 ]` が壊れる | 全コマンド |
| J4 更新通知が npm registry を参照する | `postAction` フックで `https://registry.npmjs.org/%40mimo-3%2Fslack-cli/latest` を叩き、24時間キャッシュ（`~/.slack-cli/update-notifier.json`）、タイムアウト2000ms、`CI` 定義済み / `SLACK_CLI_DISABLE_UPDATE_NOTIFIER=1` / stderr が非TTY でスキップ、例外は完全に握り潰す | 曖昧 | **選択制**。既定では機能ごと無効にする。環境変数 `SLACK_CLI_DISABLE_UPDATE_NOTIFIER` は受理し続ける（no-op）。npm 配布を継続する判断が出た時点で、キャッシュファイル名・TTL・スキップ条件・出力文言を現行のまま実装する | Rust バイナリの配布形態（cargo / Homebrew / GitHub Releases）が未定で、npm registry を参照する意味が確定しない。`isFresh` が未来時刻でも true を返すバグも配布形態と一緒に決める。無効化しても CLI 本体の挙動・終了コードには影響しない（現行も例外を握り潰す設計） | 全コマンド（実行後フック） |
| J5 `\s` の集合差 | `sanitizeSingleLineText` は JS の `\s`（スペース / `\t` / `\n` / `\r` / `\f` / `\v` / NBSP / 各種 Unicode 空白 / U+FEFF）で分割する。Rust の `char::is_whitespace` は U+FEFF を含まない | バグ（軽微） | **修正**。`char::is_whitespace() \|\| c == '\u{FEFF}'` で JS の `\s` に合わせる | 表示崩し防止が目的の関数なので、BOM を取りこぼすと目的を達しない。集合を合わせるコストはほぼゼロ | table / simple 出力の全コマンド |
| J6 `drafts.json` の JSON パース失敗が素通しで伝播する | `readDrafts` は `ENOENT` のみ `[]` にフォールバックし、`JSON.parse` の失敗はそのまま throw する（`✗ Error: Unexpected token ...` のような Node の文言が出る） | バグ | **修正**。`✗ Error: Invalid drafts file format: ~/.slack-cli/drafts.json` に置き換える。空配列へのフォールバックは**しない** | Node の内部パーサ文言が漏れると利用者が原因を特定できない。一方、空配列に倒すとファイルを上書きした瞬間に既存ドラフトが全消えするため、エラーで止めるのが正しい | `draft save` / `draft list` / `draft show` / `draft send` / `draft delete` |
| J7 `last_read` フィールドに別の意味の値を詰める | `unread` の全チャンネルモード経路1（`search.messages`）では、チャンネルごとの**最新マッチの ts** を `Channel.last_read` に入れている。本来の `last_read`（最後に既読にした位置）とは意味が違う。table の `Last Message` 列がこの値を表示している | バグ | **修正**。内部型のフィールドを `last_message_ts` として分ける。table の列見出し `Last Message` と表示値は変えない | 型を流用すると Rust では意味の取り違えがコンパイラに検出されず残る。JSON 出力にはこのフィールドが出ていない（`channel` / `channelId` / `unreadCount` のみ）ので、互換への影響は無い | `unread`（内部実装のみ） |
| J8 `process.exit(1)` でバッファが flush されない可能性 | Node の `process.exit` はバッファ未フラッシュのまま落ちうる。Rust の `std::process::exit` も同じくデストラクタが走らない | バグ | **修正**。終了前に stdout / stderr を明示的に flush する | パイプ出力時に末尾が欠ける事故を防ぐ。出力内容そのものは変わらない | 全コマンド |
| J9 `files.uploadV2` は SDK 固有のヘルパで実 API ではない | Slack Web API に `files.uploadV2` は存在せず、Node SDK が `files.getUploadURLExternal` → 外部 URL への PUT → `files.completeUploadExternal` をまとめたもの。TS 側のレスポンス処理は `{ ok, files: [{ ok, error, files: [...] }] }` という入れ子構造（SDK が包み直した形）を前提にしている | 仕様 | **そのまま再現（出力のみ）**。3段構成を自前実装したうえで、`upload --format json` の出力構造 `{ channel, files: [{ id, name, title, permalink, permalink_public, url_private }] }` は現行に合わせる | 既定方針1（JSON 出力の構造）。内部の API 呼び出し順は観測できないので自由に組めるが、出力 JSON は契約 | `upload` |
| J10 `stars.*` API を使っている | `bookmark add` / `list` / `remove` は `bookmarks.*` ではなく `stars.add` / `stars.list` / `stars.remove` を呼ぶ。Slack 側での現在のサポート状況・必要スコープはソースから確認できていない（仕様書も「不明」） | 曖昧 | **そのまま再現**（コマンド名・API とも変更しない）。移植の実装着手前に Slack API の現況を確認する | 現行と同じ API を叩く限り、動くものは動き、動かないものは同じエラーが出る。`bookmarks.*` への乗り換えは機能追加の話であって移植の範囲外。**推測**: Slack が `stars.*` を廃止済みなら現行 TS 版も既に動いていないはずで、その場合は移植の対象から外す判断もありうる | `bookmark add` / `bookmark list` / `bookmark remove` |

---

## 11. 判断が割れそうな論点

以下は方針を決めたものの、逆の判断も十分に成り立つ。実装着手前に合意を取るべき箇所。

### 論点1: `--limit` の意味を「総件数の上限」に統一するか（G5 / G6）

現行の `--limit` は、コマンドによって「1ページあたりの件数」（`channels`）「1ページだけ取る上限」（`members` / `bookmark list` / `scheduled list`）
「表示件数の上限」（`unread` 全チャンネルモード）「取得打ち切り件数」（`users list`）と、5通りの意味を持っている。

- **採用した案**: 「取得・表示する総件数の上限」に統一し、`channels` の既定値のみ無制限に変える。
- **逆の案**: 現行の非対称をそのまま再現する。`--limit` の意味がコマンドごとに違うことを受け入れる。
- **判断の分かれ目**: 統一すると `members -c foo --limit 100` が「100件取って打ち切る」から「100件目まで確実に取る」に変わる（現行はチャンネルに200人いても100人しか返らないので、結果は同じ）。一方 `bookmark list --limit 200` は現行が1ページ（Slack の既定件数）しか返さないのに対し、修正後は200件返るようになる。**出力件数が増える方向の変更**なので、件数を数えているスクリプトに影響する。

### 論点2: `channels --format json` の `created` を実時刻に直すか（D5）

- **採用した案**: `2019-04-01T00:00:00Z`（偽の時刻）を、実際の作成時刻の RFC3339 に直す。
- **逆の案**: 情報を捨てて `T00:00:00Z` を付け直す現行の奇妙な変換をそのまま実装する。
- **判断の分かれ目**: JSON のキー名も型（文字列）も変わらないので `jq '.[].created'` は動き続けるが、**値が変わる**。日付部分だけを取り出して比較しているスクリプトは動き、時刻を含めて完全一致で比較しているスクリプトは壊れる。仕様書は「互換維持ならこの奇妙な変換をそのまま実装する」と両論併記にしている。

### 論点3: `config clear` を存在しないプロファイルでエラーにするか（H3）

- **採用した案**: `Profile "xxx" not found` でエラー終了（コード1）。`config use` と揃える。
- **逆の案**: 現行どおり成功扱い。削除操作の冪等性を保つ。
- **判断の分かれ目**: `slack-cli config clear --profile temp || true` のような、あるいは「消えていればよい」前提のクリーンアップスクリプトが壊れる。削除系コマンドを冪等にしておくのは一般的な設計判断でもあり、「非対称は検証ありに揃える」という既定方針が常に正しいとは限らない場面。

---

## 12. リリースノート用: 移植で意図的に変える挙動

以下はそのまま貼れる形で書いた。Rust版 v1.0.0 のリリースノートの「Breaking changes」節を想定している。

---

### Breaking changes

TypeScript版から Rust版への移植にあたり、以下の挙動を意図的に変更しました。
**コマンド名・フラグ名・JSON出力のキー構造・終了コード（成功0 / 失敗1）は変更していません。**
設定ファイル `~/.slack-cli/config.json` とトークンの暗号化形式も完全互換なので、**再ログインは不要**です。

#### 引数の検証が厳しくなりました

- `--limit` / `--number` / `--page` に数値以外を渡すとエラーで終了するようになりました。
  従来は `--limit abc` が黙って無効な値としてSlack APIに送られたり、`--limit 12abc` が `12` と解釈されたりしていました。
- `--format` の検証が全コマンドに入りました。従来 `channels` / `unread` / `download` の3コマンドだけは
  `--format xml` のような不正な値を黙って `table` として扱っていました。
- `users list --limit abc` は従来「0件・成功（終了コード0）」でしたが、エラー（終了コード1）になります。
- `config clear` は存在しないプロファイル名を指定するとエラーになります（従来は成功扱いでした）。
  クリーンアップ用のスクリプトで使っている場合はご確認ください。

#### 日付・時刻の表示と解釈が変わりました

- `unread` の時刻表示と `channel info` の `Created:` が、実行環境のロケール・タイムゾーンに依存しなくなりました。
  すべて UTC の `YYYY-MM-DD HH:MM:SS` / `YYYY-MM-DD` 形式で出力します（`history` の表示形式に統一）。
- `--since` / `--at` が受け付ける日時形式を明示しました。RFC3339 / ISO 8601、`YYYY-MM-DD HH:MM[:SS]`、
  `YYYY-MM-DD`、Unix秒（全桁数字）です。従来受理されていた `"Jan 1, 2024"` のような形式は使えません。
- **タイムゾーンを書かない日時は、すべてローカルタイムゾーンとして解釈します。** 従来は
  `2024-01-01`（日付のみ）が UTC、`2024-01-01T00:00:00`（日時）がローカル、という非対称な解釈でした。
- `channels --format json` の `created` が、実際の作成時刻を含む RFC3339 になりました。
  従来は日付だけを残して時刻を `T00:00:00Z` に固定していました。

#### 表形式の出力が変わりました

- `--format table` の列が、日本語や絵文字を含む値でも揃うようになりました。
  従来は文字数（UTF-16）でパディングしていたため、日本語のチャンネル名や氏名で列がずれていました。
- 長い値の切り詰めも表示幅ベースになり、絵文字を途中で分断しなくなりました。
- `draft list` / `scheduled list` / `users list` / `users presence` / `pin list` の table 出力が、
  他コマンドと同じ体裁（ヘッダ + 罫線）になりました。従来は Node.js の `console.table` を使っていたため、
  `(index)` 列や値を囲むシングルクォートが付いていました。
- **表形式は人間が読むための出力です。機械処理には `--format json` を使ってください。**

#### `--format json` の出力が変わりました

- 結果が0件のとき、`--format json` でも JSON（空配列など）を出すようになりました。
  従来は `No channels found` のような人間向けテキストが出ていたため、JSON としてパースできませんでした。
  対象: `channels` / `members` / `users list` / `usergroups list` / `pin list` / `search` /
  `bookmark list` / `canvas read` / `canvas list`
- `unread` の JSON で、ユーザー名が解決できない場合の値が `unknown` / ユーザーID から
  `Unknown User` / `Bot` / `Unknown` に変わりました（`history` の規則に統一）。
- `unread --count-only --format json` が JSON を出すようになりました。
  従来は `--count-only` が `--format` の指定を上書きしてテキストを出していました。

#### 動くようになったもの

- **チャンネル名がより多くのコマンドで使えるようになりました。** `send` / `send-ephemeral` /
  `draft send` / `bookmark add` / `bookmark remove` は、チャンネル名を渡してSlackが認識できなかった場合に
  自動でチャンネル一覧から名前を解決して再試行します。
- **`invite` でユーザー名が使えるようになりました。** 従来はユーザーIDのみ有効でした。
- **`--format simple` が `upload` / `users info` / `users lookup` で動くようになりました。**
  従来はこの3コマンドだけ `simple` を指定しても `table` と同じ出力になっていました。
- **一覧取得のページネーションが効くようになりました。** `members` / `bookmark list` /
  `canvas list` / `scheduled list` は、従来1ページ目しか取得していませんでした。
- `channels --limit` が実際に件数の上限として効くようになりました（従来は1ページあたりの件数にしか
  影響せず、全件が返っていました）。**既定値は「無制限」なので、`--limit` を指定していない場合の
  出力は従来と変わりません。**

#### レート制限への対応が変わりました

- HTTP 429 と `Retry-After` ヘッダを見て自動でリトライするようになりました（最大3回、
  指数バックオフ + ジッタ、待機の上限60秒）。従来はエラーメッセージに `rate limit` という文字列が
  含まれるかどうかで判定し、一部のコマンドでのみ固定5秒待っていました。
- すべてのAPI呼び出しが同時実行数3の制限を通るようになりました（未読スキャンのみ15）。

#### セキュリティ

- **`download --url` は、Slackのファイル配信ドメイン以外に対して認証トークンを送らなくなりました。**
  従来はどんなURLに対しても `Authorization: Bearer <token>` を付けていたため、
  Slack以外のURLを渡すとトークンが漏れる状態でした。
- リダイレクトの追従先ホストが変わった場合、認証ヘッダを引き継がなくなりました。
  API呼び出しではリダイレクトを追従しません。
- **成功メッセージのエスケープシーケンス除去漏れを修正しました。** `join` / `leave` / `invite` /
  `channel set-topic` / `channel set-purpose` / `reaction add` / `reaction remove` / `pin add` /
  `pin remove` / `bookmark add` / `bookmark remove` の成功メッセージは、従来 `--channel` などの
  引数をそのまま端末に出力していました。

#### エラーメッセージの表示

- バリデーションエラーの表示が `error: Error: <内容>` から `✗ Error: <内容>` に統一されました。
  従来は `Error:` が二重に前置されていました。終了コード（1）は変わりません。
- 必須オプションの欠落と相互排他の違反も同じ形式で出るようになりました。
  従来はコマンドによって `error: required option '-c, --channel <channel>' not specified` /
  `error: Error: --channel is required` / `✗ Error: Cannot use both --id and --name` の3種類がありました。
- `reaction` / `pin` の `--timestamp` の形式エラーが `Invalid message timestamp format` になりました
  （従来は `Invalid thread timestamp format` と表示されていました）。

#### 表示の細かい修正

- チャンネル名の `#` が二重に付かなくなりました（`send -c "#general"` の成功メッセージが
  `##general` から `#general` に）。チャンネルIDを渡した場合は `#` を付けません。
- `reaction --emoji :tada:` の成功メッセージでコロンが二重にならなくなりました（`::tada::` → `:tada:`）。
- `config profiles` がトークンを復号してからマスクするようになりました。
  従来は暗号化された文字列をマスクしていたため、`v2-****-****-3f2a` という無意味な表示でした。
- `invite --force` で一部のユーザーの招待に失敗した場合と、`join` で既に参加済みだった場合に、
  警告を標準エラー出力に表示するようになりました（標準出力と終了コードは変わりません）。

#### 挙動が変わらないもの（念のため）

- `config get` はトークンを復号するだけで、設定ファイルを書き換えなくなりました
  （従来は古い形式のトークンを読むと再暗号化して書き戻していました）。
  形式の移行は `config set` などの書き込み時に行われるため、**利用者の操作は不要です。**
- `--format json` の `text` フィールドはメンションを `<@U0123ABCD>` のまま出力します（従来どおり）。
  表示名に置換した値が必要な場合は `--format table` / `simple` を使ってください。
- `unread --mark-read` は `--limit` の指定に関わらずすべての未読チャンネルを既読にします（従来どおり）。
- `download` は出力先のファイルを確認なしで上書きします（従来どおり）。
- 終了コードは成功が0、失敗が1のみです（従来どおり）。エラーの種類による細分化はしていません。

#### 既知の制限

- 設定ファイル・ドラフトファイルの読み書きにロックがありません。同じファイルを複数のプロセスから
  同時に更新すると、後から書き込んだ側の内容だけが残ります（TypeScript版と同じ）。
- 更新通知（新しいバージョンのお知らせ）は現時点で無効です。
  `SLACK_CLI_DISABLE_UPDATE_NOTIFIER` 環境変数は引き続き受け付けます（何もしません）。
