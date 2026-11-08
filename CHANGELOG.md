# Changelog

## Unreleased

- `upload` の `--channel` を任意にし、省略時はどこにも共有しない非公開アップロードに対応。
- `upload --blocks` / `--blocks-file` で本文とファイルをまとめて投稿できるように変更。
- `send --image`（複数指定可）と `--image-alt` を追加。画像の処理完了を待って本文とまとめて投稿。
- 画像付き送信の `invalid_blocks` を最大3回再試行。画像と予約送信の併用は書き込み前に拒否。
- 画像付き dry-run はローカル検証と投稿内容の表示のみ実行。
