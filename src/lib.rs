//! slack-cli の中身。バイナリ（`src/main.rs`）は薄いディスパッチャで、実体はここにある。
//!
//! モジュールの役割:
//!
//! - [`cli`]    — clap のコマンド定義と各コマンドの `run()`。HTTP を直接触らない
//! - [`client`] — Slack Web API クライアント（認証ヘッダ・リトライ・ページング）
//! - [`config`] — `~/.slack-cli/config.json` の読み書きとトークン暗号化
//! - [`output`] — `serde_json::Value` を table / json / yaml / csv / tsv / id-only で書き出す
//! - [`error`]  — エラー階層と終了コード
//!
//! 全層が `serde_json::Value` を流す設計にしてある。型を厳密に切らない代わりに
//! フォーマッタが完全に汎用化でき、エンドポイントを足すコストが下がる。

pub mod cli;
pub mod client;
pub mod config;
pub mod error;
pub mod output;
