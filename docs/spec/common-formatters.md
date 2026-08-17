# 出力整形（formatters）仕様（Rust移植用）

`common-format.md` で「範囲外」として送りにした出力整形の実体をここに書く。

対象として実際に読んだファイル:

- `src/utils/formatters/` 配下 10ファイル
  （`base-formatter.ts` / `bookmark-formatters.ts` / `channel-formatters.ts` /
  `channel-info-formatters.ts` / `channels-list-formatters.ts` / `history-formatters.ts` /
  `members-formatters.ts` / `message-formatters.ts` / `reminder-formatters.ts` /
  `search-formatters.ts`）
- `src/utils/command-support.ts`（`renderByFormat`）
- `src/utils/option-parsers.ts` の `parseFormat`、`src/utils/validators.ts` の format 検証
- `console.table` を呼ぶ 5ファイル
  （`src/commands/scheduled.ts` / `draft.ts` / `usergroups.ts` / `pin.ts` / `users.ts`）
- 整形をコマンド内に直書きしている `src/commands/canvas.ts` / `download.ts` / `upload.ts`

**重要な前提**: このリポジトリの「table 形式」は、**単一の共通テーブル描画器を持たない**。
実装は 3系統に分かれており、それぞれ列幅の決め方も罫線も違う。

| 系統 | 実装場所 | 罫線 | 列幅 |
| --- | --- | --- | --- |
| A. 固定幅パディング系 | `formatters/` の一部（bookmark / reminder / members / channels-list / channel） | ヘッダ下に `─` の1本線のみ（または線なし） | **ソースにベタ書きの定数**。データを見ない |
| B. 非テーブル系（table という名の装飾出力） | `formatters/` の残り（history / search / message / channel-info） | 罫線なし | 列という概念なし。ラベル + 色 |
| C. `console.table` 系 | `commands/` の5ファイル | Node の box-drawing 罫線 | Node がデータから自動計算 |

---

## 1. フォーマット指定の分岐点

### 1.1 `base-formatter.ts` の骨格

```ts
export interface BaseFormatter<T> { format(data: T): void; }

export abstract class JsonFormatter<TInput, TOutput = unknown> extends AbstractFormatter<TInput> {
  protected abstract transform(data: TInput): TOutput;
  format(data: TInput): void {
    console.log(JSON.stringify(sanitizeTerminalData(this.transform(data)), null, 2));
  }
}

export interface FormatterMap<T> {
  table: BaseFormatter<T>;
  simple: BaseFormatter<T>;
  json: BaseFormatter<T>;
  [key: string]: BaseFormatter<T>;
}

export class FormatterFactory<T> {
  create(format: string = 'table'): BaseFormatter<T> {
    return this.formatters[format] || this.formatters.table;
  }
}
```

分岐規則:

- **未知のフォーマット名は table にフォールバックする**（例外を投げない）。
- 引数省略時のデフォルトも `'table'`。
- `FormatterMap` はインデックスシグネチャを持つので、`table`/`simple`/`json` 以外の
  追加キーを持てる。実際に使われているのは `channel-formatters.ts` の `count` のみ。
- JSON 出力は共通で `JSON.stringify(..., null, 2)`。**インデント2スペース、末尾改行は
  `console.log` による1個**。キー順は `transform` が返すオブジェクトのリテラル順。

### 1.2 `command-support.ts` の `renderByFormat`（`formatters/` を使わない側の分岐）

```ts
export function renderByFormat<T>(options, data, renderers: {table, simple?, json?}): void {
  const format = parseFormat(options.format);        // format || 'table'
  if (format === 'json') {
    if (renderers.json) { renderers.json(data); return; }
    console.log(JSON.stringify(sanitizeTerminalData(data), null, 2));   // 既定のJSON
    return;
  }
  if (format === 'simple' && renderers.simple) { renderers.simple(data); return; }
  renderers.table(data);
}
```

- `json` レンダラ未指定なら**データをそのまま整形して出す**フォールバックがある。
  `formatters/` 側（`transform` で明示的にキーを組み替える）とは挙動が違う。
- `simple` レンダラ未指定なら table に落ちる。
- 未知のフォーマット名も table に落ちる。

### 1.3 バリデーション

`validators.ts` に **2つの別々の許可リスト**がある。

| 場所 | 許可値 | 実際に使われているか |
| --- | --- | --- |
| `optionValidators.format`（preAction フック） | `['table','simple','json']` | **使われている**。全コマンドの `.hook('preAction', createValidationHook([optionValidators.format]))` |
| `optionValidators.outputFormat` | `['table','simple','json','compact']` | 参照箇所なし（デッドコード） |

- したがって **`compact` 形式は実装されていない**。`outputFormat` バリデータに文字列として
  残っているだけで、`compact` を受け付けるコマンドも、`compact` フォーマッタも存在しない。
  （`src/` 全体を `compact` で grep しても `validators.ts:115` の1件のみ）
- CLI ヘルプ文言はすべて `'Output format: table, simple, json'`、既定値 `'table'`。
- `parseFormat(format?, defaultFormat = 'table')` は `format || defaultFormat` だけ。
  空文字列は falsy なので `'table'` になる。

---

## 2. table 形式の描画ロジック（系統A: 固定幅パディング）

共通する性質:

- **列幅はソースにベタ書きの定数**。データの最大長を見ない。
- パディングは `String.prototype.padEnd`（= **UTF-16 コードユニット数**基準。
  日本語や絵文字では表示幅とずれる。設計判断ポイント）。
- 値が列幅を超えたら `padEnd` は何もしないので**列がずれて右に押し出される**。
  切り詰めを行うのは一部の列のみ（下記）。
- **セル間の縦罫線・外枠は一切ない**。ヘッダ直下に `─`（U+2500）の水平線を引くだけ。
- **空リスト時の挙動: ヘッダと水平線だけが出力され、本体は0行**。
  ただし実際には**呼び出し側が空リストを先に握りつぶす**ため、通常は到達しない（§6）。

### 2.1 `bookmark-formatters.ts`

| 列 | 幅 | 元データ | 切り詰め |
| --- | --- | --- | --- |
| `Channel` | 16 | `item.channel` | なし |
| `Timestamp` | 20 | `item.message?.ts` | なし |
| `Text` | 40 | `item.message?.text` | **`slice(0, 38)`**（= `textWidth - 2`）。省略記号は付かない |
| `Saved At` | 26 | `new Date(date_create*1000).toISOString()` | なし |

- ヘッダ: `'Channel'.padEnd(16) + 'Timestamp'.padEnd(20) + 'Text'.padEnd(40) + 'Saved At'.padEnd(26)`
  → 装飾なし（chalk を使っていない）。
- 区切り線: `'─'.repeat(16+20+40+26)` = **`─` × 102**。
- 行: 各セルを `padEnd` して**セパレータなしで単純連結**（`${channel}${ts}${text}${savedAt}`）。
- セル値は `sanitizeSingleLineText`（改行・タブを空白1個に潰す）。`Saved At` のみ非サニタイズ。

### 2.2 `reminder-formatters.ts`

| 列 | 幅 | 切り詰め |
| --- | --- | --- |
| `ID` | 14 | なし |
| `Text` | 30 | **`slice(0, 28)`** |
| `Time` | 26 | なし（`toISOString()`） |
| `Status` | 10 | なし（`completed` / `pending`） |

- 区切り線: `─` × 80。行は連結、セパレータなし。ヘッダ装飾なし。
- `getStatus(complete_ts)` = `complete_ts > 0 ? 'completed' : 'pending'`。

### 2.3 `members-formatters.ts`

| 列 | 幅 | 備考 |
| --- | --- | --- |
| `ID` | 17 | `padEnd(17)` |
| `Name` | 17 | `padEnd(17)` |
| `Real Name` | — | パディングなし（最終列） |

- ヘッダは**ベタ書きの文字列** `'ID                Name              Real Name'`。
  ここが上の `padEnd(17)` と**ずれている**: ベタ書きヘッダは `ID` + 16空白（= 18桁目から `Name`）、
  `Name` + 14空白（= 実質18+18=…）。実際の行は `id.padEnd(17) + ' ' + name.padEnd(17) + ' ' + realName`
  で列開始位置が 1, 19, 37。ヘッダ側は `ID`(0) → `Name`(18) → `Real Name`(36)。
  **ヘッダとデータで1桁ずれる**（データ行が各セル間に追加の空白1個を入れているため）。
- 区切り線: `'─'.repeat(60)` = `─` × 60。**列幅合計（17+1+17+1=36 + 実名長）とは無関係の固定値**。
- 切り詰めなし。

### 2.4 `channels-list-formatters.ts`

| 列 | 幅 | 切り詰め |
| --- | --- | --- |
| `Name` | `padEnd(17)` | なし |
| `Type` | `padEnd(9)` | なし |
| `Members` | `padEnd(8)` | なし |
| `Created` | `padEnd(12)` | なし |
| `Description` | 最終列 | **30文字超なら `substring(0,27) + '...'`**（唯一「…」を付ける列） |

- ヘッダはベタ書き `'Name              Type      Members  Created      Description'`。
- 区切り線: `─` × 65。
- 行: `${name} ${type} ${members} ${created} ${purpose}`（**セル間に空白1個**）。
- `type` は `mapChannelToInfo` が返す `public` / `private` / `im` / `mpim` / `unknown`。
- `created` は `formatUnixTimestamp`（UTC の `YYYY-MM-DD`、10文字）。

### 2.5 `channel-formatters.ts`（unread のチャンネル一覧）

- ヘッダ: `chalk.bold('Channel          Unread  Last Message')` — **これだけ太字**。
- 区切り線: `─` × 50（**chalk なし**）。
- 行: `${name.padEnd(16)} ${count.padEnd(6)}  ${lastRead}`（空白1個 + 空白2個）。
- `lastRead` は `channel.last_read` があれば `formatSlackTimestamp`（**ロケール/TZ依存**）、
  なければ `'Unknown'`。
- 切り詰めなし。

### 2.6 `count` フォーマッタ（`channel-formatters.ts` のみの第4の形式）

`createChannelFormatter(format, countOnly)` は `countOnly === true` のとき
**format 引数を無視して `count` フォーマッタを返す**。

```
<チャンネル名>: <件数>      ← 各チャンネル1行、色なし
Total: <合計> unread messages   ← chalk.bold
```

---

## 3. table 形式の描画ロジック（系統B: 罫線なしの装飾出力）

「table」という名前だが**表ではない**。列も罫線もない。

### 3.1 `history-formatters.ts` (`TableHistoryFormatter`)

```
<空行>
Message History for #<channelName>:            ← chalk.bold（先頭に \n を含む）
（messages.length === 0 なら）No messages found  ← chalk.yellow、ここで return
<空行>
[YYYY-MM-DD HH:MM:SS] <username>               ← [時刻] は chalk.gray、username は chalk.cyan
<本文>                                          ← 色なし。メンション <@U…> は @name に展開
  📎 <name, mimetype, size> <url>              ← ラベル部は chalk.yellow、URL は chalk.blue
<permalink>                                     ← chalk.blue
<空行>
（メッセージごとに繰り返し）
✓ Displayed N message(s)                        ← chalk.green
```

- 時刻は `formatTimestampFixed`（**UTC 固定**、`YYYY-MM-DD HH:MM:SS`）。
- ユーザー名は `resolveUsername`: `user` があれば `users.get(user) || 'Unknown User'`、
  なければ `bot_id` があれば `'Bot'`、それもなければ `'Unknown'`。
- 本文が空なら `'(no text)'`。
- ファイルサイズ: `<1024` → `N B` / `<1MB` → `N.N KB` / それ以上 → `N.N MB`（`toFixed(1)`）。
- ファイルのURLは `url_private_download || url_private || permalink || ''`。空なら URL 部を出さない。
- 絵文字リテラル `📎` と `✓` をそのまま出す。

### 3.2 `search-formatters.ts` (`TableSearchFormatter`)

```
<空行>
Search results for "<query>" (<totalCount> matches)   ← chalk.bold
（matches.length === 0 なら）No messages found         ← chalk.yellow、ここで return
（pageCount > 1 のときのみ）Page <page>/<pageCount>     ← chalk.gray
<空行>
[<timestamp>] #<channel> <username>   ← [時刻]=gray, #channel=blue, username=cyan
<本文>                                 ← 色なし
<permalink>                            ← chalk.gray（history は blue、こちらは gray）
<空行>
Displayed N of M match(es)             ← chalk.green（✓ は付かない）
```

- チャンネル表記: `match.channel.name` があれば `#name`、なければ `match.channel.id`、
  それもなければ `'unknown'`（`#` は付かない）。
- ユーザー名: `match.username || match.user || 'Unknown'`。
- 時刻: `match.ts` があれば `formatTimestampFixed`、なければ空文字列（`[]` と出る）。

### 3.3 `message-formatters.ts` (`TableMessageFormatter`、unread の1チャンネル表示)

```
#<channel>: <N> unread messages          ← chalk.bold
（countOnly でなく messages が1件以上のとき）
<空行>
<timestamp> <author>                     ← timestamp=chalk.gray, author=chalk.cyan（[] で囲まない）
<本文>
<空行>
（繰り返し）
Showing latest X of Y unread messages    ← chalk.gray。X<Y のときのみ
```

- 時刻は `formatSlackTimestamp`（`toLocaleString()`。**ロケール/TZ依存**）。
  history/search が UTC 固定なのに対しここだけ違う。
- author は `users.get(message.user) || message.user`、`user` 自体がなければ `'unknown'`。
- 未読件数は `totalUnreadCount ?? channel.unread_count ?? 0`。
- **空リストでも「No messages found」は出ない**。ヘッダ行だけ出て終わる。

### 3.4 `channel-info-formatters.ts` (`TableChannelInfoFormatter`)

```
<空行>
Channel Info: #<name>          ← chalk.bold
<空行>
  ID:       <id>               ← ラベルのみ chalk.gray、値は色なし
  Name:     <name>
  Private:  Yes|No
  Archived: Yes|No
  Members:  <n>                ← num_members が undefined でないときのみ
  Created:  <toLocaleDateString()>   ← ロケール/TZ依存
<空行>
  Topic:    <topic|(not set)>
  Purpose:  <purpose|(not set)>
<空行>
```

- 先頭2スペースのインデント固定。ラベルは `ID:`(3) 〜 `Archived:`(9) を右側スペースで
  揃えたベタ書き（`'ID:'` の後に7スペース、`'Name:'` の後に5スペース、等）。
  **`padEnd` ではなくソース上の空白リテラル**。

---

## 4. simple 形式の描画ロジック

原則は **1レコード1行、色なし、TAB または空白区切り**。ヘッダ行も区切り線もない。

| フォーマッタ | 出力 |
| --- | --- |
| bookmark | `channel \t ts \t text \t savedAt`（TAB区切り、**text は切り詰めなし**） |
| reminder | `id \t text \t time \t status`（TAB区切り、切り詰めなし） |
| members | `id \t name \t realName`（TAB区切り） |
| channels-list | チャンネル名のみ1行（`sanitizeSingleLineText(channel.name)`） |
| channel（unread一覧） | `<name> (<unreadCount>)` |
| channel-info | `<name> (<id>)` / `Topic: …` / `Purpose: …` / `Members: …` の最大4行 |
| history | `[<UTC時刻>] <username>: <text>[📎 f1, f2] <permalink>`（ファイル・permalink は該当時のみ） |
| search | `[#channel] <username> (<timestamp>): <text>`、末尾に `... and N more match(es)`（`totalCount > matches.length` のとき） |
| message（unread単一） | `#<channel> (<n>)` の後に `[<localtime>] <author>: <text>` |
| scheduled（commands/） | `<post_at> <channel_id> <id> <text>`（**空白区切り**、TABでない） |
| draft（commands/） | `<id> <createdAt> <target> <message>`（空白区切り、切り詰めなし） |
| usergroups（commands/） | `<id> \t @<handle> \t <name>`（TAB区切り、`@` が付く） |
| pin（commands/） | `<created> <ts> <text>`（空白区切り） |
| users list（commands/） | `<id> \t <name> \t <real_name>` + email があれば ` <email>` を追記 |
| users presence | `<userId> \t <presence>` |
| canvas read | `<sectionId> \t <text>` |
| canvas list | `<canvasId> \t <name>` |
| download | ダウンロード先パスのみ1行 |

simple 形式で **空リストのときに何か出すのは history と search だけ**（どちらも `No messages found`、色なし）。他は0行。

---

## 5. compact 形式

**存在しない。** §1.3 の通り `validators.ts` の未使用バリデータに文字列としてだけ残っている。
Rust 側で実装する必要はない（互換性の観点でも、TS 版で `--format compact` を渡すと
`optionValidators.format` が `Invalid format 'compact'. Must be one of: table, simple, json`
を返して弾く）。

---

## 6. `console.table` を使っている箇所と実際の見え方

### 6.1 呼び出し箇所（全6箇所・5ファイル）

| ファイル:行 | 関数 | 列（オブジェクトのキー順） |
| --- | --- | --- |
| `commands/scheduled.ts:24` | `renderTable` | `id`, `channel`, `post_at`, `text` |
| `commands/draft.ts:44` | `renderTable` | `id`, `target`, `created_at`, `message` |
| `commands/usergroups.ts:20` | `renderUsergroupTable` | `id`, `handle`, `name`, `description`, `user_count` |
| `commands/pin.ts:24` | `renderTable` | `type`, `created`, `created_by`, `ts`, `text` |
| `commands/users.ts:29` | `renderUserTable` | `id`, `name`, `real_name`, `email`, `is_bot`, `deleted` |
| `commands/users.ts:72` | `renderPresenceTable` | `user`, `presence`（常に1行） |

いずれも `console.table(sanitizeTerminalData(rows))` の形。`rows` は
プレーンオブジェクトの配列で、値は事前に `sanitizeTerminalText` / `sanitizeSingleLineText` 済み。

前処理の差分:

- `draft.ts` は `message` を **60文字超なら `slice(0,60) + '...'`** に切り詰める。
  他の5箇所は**切り詰めなし**（`pin.ts` の `text`、`scheduled.ts` の `text`、
  `usergroups.ts` の `description` は全文がそのまま列になる）。
- `users.ts` の `is_bot` / `deleted` は真偽値ではなく **`'Yes'` / `'No'` の文字列**に変換済み。
- `usergroups.ts` の `user_count` は `usergroup.user_count ?? ''` なので**数値または空文字列**。
  同じ列に数値と文字列が混ざり得る。

### 6.2 Node の `console.table` の罫線仕様（実測: Node v24.15.0）

実測した出力（`[{id:'D001',target:'#general',created_at:'2026-01-01T00:00:00Z',message:'hello'},…]`）:

```
┌─────────┬────────┬────────────┬────────────────────────┬─────────┐
│ (index) │ id     │ target     │ created_at             │ message │
├─────────┼────────┼────────────┼────────────────────────┼─────────┤
│ 0       │ 'D001' │ '#general' │ '2026-01-01T00:00:00Z' │ 'hello' │
│ 1       │ 'D2'   │ '@ミモ'    │ '2026-01-02T00:00:00Z' │ ''      │
└─────────┴────────┴────────────┴────────────────────────┴─────────┘
```

規則:

1. **罫線文字**（U+250x/251x/252x/253x の box-drawing、単線）:
   `┌ ┬ ┐` / `├ ┼ ┤` / `└ ┴ ┘` / 縦 `│` / 横 `─`。
2. **先頭に `(index)` 列が必ず入る**。配列を渡した場合は 0 起点の**配列インデックス**が値になる。
   これは TS 側のデータには存在しない、Node が勝手に足す列。
3. **列は全行のキーの和集合**、初出順。ある行に存在しないキーのセルは**空**（空文字列）。
4. **セルの値は `util.inspect` 経由**。したがって
   - 文字列は **シングルクォートで囲まれる**（`'D001'`）。空文字列は `''`。
   - 数値・真偽値は裸（`5`、`true`）。`null` は `null`、`undefined` は `undefined` と表示。
   - `maxArrayLength: 3`, `breakLength: Infinity` のオプションで inspect される。
5. **列幅 = その列のヘッダとセルの表示幅の最大値**。データ依存で自動決定。
   **上限なし・折り返しなし・切り詰めなし**。80文字の文字列があれば列も80+2桁になる。
6. **セル両脇に空白1個ずつのパディング**が入る（区切り線はセル幅+2）。
7. **中身は左寄せ**（Node v24 実測。旧バージョンの Node は中央寄せだったので**バージョン依存**）。
8. **表示幅は East Asian Width を考慮**して計算される（`'あいうえお'` は 12幅として扱われ、
   ASCII 行と正しく揃う）。`padEnd` 系（§2）とはここが決定的に違う。
9. **空配列**を渡すと `(index)` 列だけのヘッダが出る:
   ```
   ┌─────────┐
   │ (index) │
   ├─────────┤
   └─────────┘
   ```
10. 出力先は **stdout**（`console.log` と同じ）。末尾に改行1個。

### 6.3 空リスト時の実際の挙動

`console.table([])` に到達することは実運用ではない。**呼び出し側が空判定して固定文言を出し、
early return する**ため。

| コマンド | 文言 |
| --- | --- |
| `scheduled list` | `No scheduled messages found` |
| `draft list` | `No drafts found` |
| `usergroups list` | `No usergroups found` |
| `usergroups members` | `No members found` |
| `pin list` | `No pinned items found` |
| `users list` | `No users found` |
| `members` | `No members found` |
| `channels` | `No channels found`（`ERROR_MESSAGES.NO_CHANNELS_FOUND`） |
| `bookmark list` | `No saved items found` |
| `reminder list` | `No reminders found` |

いずれも **chalk なし・素の `console.log`**。`--format json` を指定していても**この文言が
先に出て JSON は出ない**（早期 return が format 分岐より手前にある）。JSON パーサに食わせる
用途では壊れる挙動だが、TS 版の仕様としてはこうなっている。

---

## 7. chalk による色付け規則

`chalk` 5.6.2。`src/` 全体での使用回数は次の通り（`chalk.X` の出現数）。

| 関数 | 回数 | 用途 |
| --- | --- | --- |
| `chalk.green` | 33 | **成功メッセージ**。ほぼすべて `✓ …` の形 |
| `chalk.gray` | 17 | **時刻・ラベル・補助情報**（`[時刻]`、`ID:` 等のラベル、ページ番号、truncate 通知） |
| `chalk.yellow` | 9 | **警告・空結果・ファイル添付ラベル**（`No messages found`、`📎 …`） |
| `chalk.bold` | 9 | **見出し行 / ヘッダ行 / 合計行** |
| `chalk.cyan` | 8 | **ユーザー名**（および canvas の ID ラベル） |
| `chalk.blue` | 3 | **URL / permalink / チャンネル名**（history のファイルURL・permalink、search のチャンネル） |
| `chalk.red` | 1 | エラー |

フィールドごとの規則（formatters 内）:

| フィールド | 色 | 出現箇所 |
| --- | --- | --- |
| 見出し（`Message History for #x:` / `Search results for …` / `Channel Info: #x` / `#ch: N unread messages`） | `bold` | history / search / channel-info / message |
| テーブルヘッダ行 | `bold` | channel-formatters のみ（他のテーブルヘッダは無装飾） |
| `Total: N unread messages` | `bold` | channel count |
| タイムスタンプ `[…]` | `gray` | history / search / message |
| 属性ラベル（`ID:` `Name:` `Private:` …） | `gray` | channel-info |
| `Page x/y` | `gray` | search |
| `Showing latest X of Y unread messages` | `gray` | message |
| permalink | `gray`（search）/ `blue`（history） | **同じ意味の値に別の色が当たっている** |
| ユーザー名 | `cyan` | history / search / message |
| チャンネル名 `#x`（検索結果行） | `blue` | search |
| ファイル添付ラベル `📎 …` | `yellow` | history |
| ファイルURL | `blue` | history |
| 空結果 `No messages found` | `yellow` | history / search の table のみ（simple は無色） |
| 完了行 `✓ Displayed N message(s)` / `Displayed N of M match(es)` | `green` | history / search |

- **本文テキストには一切色を当てていない**（サニタイズ済みの生テキストをそのまま出す）。
- **区切り線 `─` にも色を当てていない**。
- **simple 形式・JSON 形式では chalk を一切使わない**。
- `console.table` の6箇所も chalk を使わない（罫線は Node が出す無色の box-drawing）。
- chalk 5 は色の有効/無効を**自身が自動判定**する（TTY 判定、`NO_COLOR` / `FORCE_COLOR`
  環境変数、`--color` / `--no-color` 引数）。このリポジトリ側で `chalk.level` を明示設定
  している箇所はない。

---

## 8. 各フォーマッタが受け取るデータ構造

`formatters/` 側は全て**単一のオプションオブジェクト**を受け取る。

| フォーマッタ | 入力型 | フィールド |
| --- | --- | --- |
| bookmark | `BookmarkFormatterOptions` | `items: { type, channel, message: {text, ts}, date_create }[]` |
| channel（unread一覧） | `ChannelFormatterOptions` | `channels: Channel[]`, `countOnly?: boolean` |
| channel-info | `ChannelInfoFormatterOptions` | `channel: ChannelDetail` |
| channels-list | `ChannelsListFormatterOptions` | `channels: ChannelInfo[]`（`mapChannelToInfo` 済み） |
| history | `HistoryFormatterOptions` | `channelName: string`, `messages: Message[]`, `users: Map<string,string>`, `permalinks?: Map<string,string>` |
| members | `MembersFormatterOptions` | `members: { id, name?, realName? }[]` |
| message | `MessageFormatterOptions` | `channel: Channel`, `messages: Message[]`, `users: Map`, `countOnly: boolean`, `format: string`, `totalUnreadCount?`, `displayedMessageCount?` |
| reminder | `ReminderFormatterOptions` | `reminders: { id, text, time, complete_ts, recurring }[]` |
| search | `SearchFormatterOptions` | `query`, `matches: SearchMatch[]`, `totalCount`, `page`, `pageCount` |

- `users` は **`Map<userId, displayName>`**。Rust では `HashMap<String,String>` 相当だが、
  **JSON 出力側では Map は使われない**（`transform` が値を取り出して素の配列/オブジェクトにする）。
  `sanitizeTerminalData` は Map を「プレーンオブジェクトでない」として素通しするので、
  Map をそのまま JSON に渡すと `{}` になる。実装上そうなる経路はない。
- `MessageFormatterOptions.format` は**フィールドとして渡されているが、どのフォーマッタも
  参照していない**（デッドフィールド）。
- 呼び出し口:
  - `commands/history-display.ts` — `conversations.history` は新しい順で返るので
    **既定で `reverse()` してから**フォーマッタに渡す（`preserveOrder` 指定時は保持）。
  - `commands/unread.ts` — `channels.slice(0, limit)` してから渡す。
  - `commands/usergroups.ts:109` — members フォーマッタを流用。

---

## 9. Rust の comfy-table で 1:1 再現できない点

前提として、**comfy-table を使ってよいのは §6 の `console.table` 系6箇所だけ**。
§2 の固定幅系と §3 の装飾出力系は、comfy-table に載せた時点で見た目が別物になる。

### 9.1 `console.table` 系（comfy-table に置き換える場合）

| 論点 | Node `console.table` | comfy-table の既定 | 対処 |
| --- | --- | --- | --- |
| `(index)` 列 | **自動で必ず付く**（0起点） | ない | 自前で連番列を先頭に足す |
| 文字列のクォート | `'foo'` と**シングルクォートで囲まれる**、空文字列は `''` | 素の値 | セル値を作る段階で `format!("'{}'", s)` する。数値・真偽値は囲まない、という**型ごとの分岐**まで再現が必要 |
| `undefined` / `null` | `undefined` / `null` と文字列で出る | 概念がない | `Option` の None を `''`（キー欠落）か `undefined`（値が undefined）か**区別して**出し分ける必要がある |
| 欠落キー | 空セル | 空セル | 一致する |
| プリセット（罫線） | 単線 box-drawing、ヘッダ下だけが `├─┼─┤` | 既定は `UTF8_FULL`（**全行間に横罫線**が入る） | `presets::UTF8_FULL` + `apply_modifier(UTF8_ROUND_CORNERS)` ではなく、行区切りを消したカスタム preset が要る |
| 折り返し | **しない**（列がいくらでも横に伸びる） | 既定で `ContentArrangement::Disabled` は折り返さないが、`Dynamic` にすると折り返す | 明示的に `Disabled` にして、`set_width` を呼ばない |
| 寄せ | 左寄せ（Node v24 実測。**旧 Node は中央寄せ**） | 左寄せが既定 | 一致するが、参照する Node のバージョンを固定して決めないと基準が揺れる |
| 表示幅 | East Asian Width 考慮 | comfy-table も `unicode-width` を使うので考慮する | ほぼ一致。ただし**絵文字・ZWJ 連結・異体字セレクタでの差**は検証していない（推測: ずれる可能性がある） |
| 出力方法 | 直接 stdout に書く | `Table` の `Display` を `println!` | 末尾改行の数を合わせる |

### 9.2 §2 の固定幅パディング系（comfy-table を使ってはいけない）

- 列幅が**データではなくソースの定数**で決まる。comfy-table は最大幅から自動計算するので、
  短いデータしかない列でも TS 版は定数幅で空白を吐く。**この時点で一致しない**。
- **縦罫線も外枠もない**。comfy-table で `NOTHING` preset を使っても、
  セル間セパレータの空白数（bookmark/reminder は0、channels-list は1、channel は1と2の混在）を
  再現するにはカスタムが要る。素直に `format!("{:<width$}")` で書いたほうが一致する。
- パディングが **UTF-16 コードユニット数**基準。日本語チャンネル名では TS 版は表示上崩れる。
  Rust で `unicode-width` を使うと**「正しく揃ってしまい」TS 版と違う出力になる**。
  1:1 を取るなら `s.chars().map(|c| if c as u32 > 0xFFFF {2} else {1}).sum()`（UTF-16 長）で
  詰める必要がある（設計判断ポイント: 互換を取るか、正しく直すか）。
- `members-formatters.ts` の**ヘッダとデータ行の1桁ずれ**、区切り線の長さが列幅合計と
  無関係（members=60, channels-list=65, channel=50）な点も、そのまま写すなら**バグごと写す**判断が要る。
- 切り詰め規則が列ごとにばらばら（bookmark text は `slice(0,38)` で省略記号なし、
  channels-list description は `substring(0,27)+'...'`、他は切り詰めなし）。comfy-table の
  `set_content_arrangement` では再現できないので、セル生成時に手で切る。
  なお **`slice` / `substring` は UTF-16 コードユニット単位**なので、
  サロゲートペア（絵文字）が境界に来ると**壊れた片割れが残る**。Rust の `chars().take(n)` とは違う。

### 9.3 §3 の装飾出力系

そもそも表ではないので comfy-table の対象外。`println!` の直書きで写す。
再現時の注意点:

- **時刻の基準が3種類混在**する。history / search は `formatTimestampFixed`（UTC 固定）、
  message / channel は `formatSlackTimestamp`（`toLocaleString()`）、
  channel-info の Created は `toLocaleDateString()`。
  **後ろ2つはロケール・TZ 依存なので Rust で完全一致は原理的に不可能**（`common-format.md` §3 と同じ論点）。
- 空行の位置（見出しの前後、メッセージごとの後ろ）と、`\n` を文字列先頭に埋め込んでいる箇所
  （`chalk.bold('\nMessage History…')`）を取りこぼすと差分が出る。
- 絵文字リテラル `✓` `📎` をそのまま出す。

### 9.4 色

- chalk の**自動判定ロジック**（TTY / `NO_COLOR` / `FORCE_COLOR` / `--color` / `--no-color` /
  `CI` / `TERM=dumb` / Windows のバージョン判定）は、Rust の `owo-colors` や `console` クレートと
  優先順位が完全には一致しない。**エスケープシーケンスのバイト列レベルで一致させるなら
  chalk の判定順を写経する必要がある**（推測: 実用上は `NO_COLOR` と TTY 判定だけ合わせれば足りる）。
- chalk 5 が出す SGR コードは `gray` = `\x1b[90m`（bright black）で、
  `owo-colors` の `.black()` ではなく `.bright_black()` に対応する。ここを間違えると色が変わる。
