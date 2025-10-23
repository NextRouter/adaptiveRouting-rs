# wan-switcher

HTTP デーモンサーバーとして動作し、LAN の IP アドレスを WAN インターフェースに動的に割り当てます。

## 特徴

- **デーモン起動**: ポート 32599 で HTTP サーバーとして常駐
- **初期化**: 起動時に LAN サブネット (10.40.0.0/20) を wan0 (eth0) に紐付け
- **動的切り替え**: `/switch?ip=<IP>&nic=<wan>` エンドポイントで特定の IP のみを wan1 に切り替え
- **デフォルトルーティング**: 明示的に切り替えられていない IP は常に wan0 (eth0) 経由
- **状態確認**: `/status` エンドポイントで現在の割り当て状態を確認
- **環境変数対応**: インターフェース名を環境変数で指定可能

## 動作概要

1. 起動時に `10.40.0.0/20` 全体が `wan0 (eth0)` に割り当てられます
2. `/switch` API で個別の IP (例: `10.40.0.3`) を `wan1 (eth1)` に切り替え可能
3. 切り替えられた IP のみが wan1 経由でルーティングされます
4. その他の IP は全て wan0 経由でルーティングされます

## 必要環境

- Linux with `ip` command (iproute2)
- root 権限で実行

## ビルド

```sh
cargo build --release
```

## 使い方

### サーバー起動（デフォルト設定）

```sh
sudo ./target/release/wan-switcher
```

デフォルト設定:

- wan0: eth0
- wan1: eth1
- lan: eth2

### サーバー起動（環境変数で設定）

```sh
# カスタムインターフェース名を指定
sudo WAN0=enp0s3 WAN1=enp0s4 LAN=enp0s5 ./target/release/wan-switcher
```

起動時に LAN サブネット全体 (10.40.0.0/20) が wan0 に紐付けられます。

### IP の切り替え

**例: 10.40.0.3 を wan1 に割り当てる**

```sh
curl "http://localhost:32599/switch?ip=10.40.0.3/20&nic=wan1"
```

または

```sh
curl "http://localhost:32599/switch?ip=10.40.0.3&nic=wan1"
```

レスポンス例:

```json
{
  "status": "success",
  "message": "Switched 10.40.0.3/32 to wan1 (eth1)"
}
```

この操作により、`10.40.0.3` のみが wan1 (eth1) 経由でルーティングされるようになります。
その他の `10.40.0.0/20` 内の IP は引き続き wan0 (eth0) 経由です。

### 現在の状態確認

```sh
curl "http://localhost:32599/status"
```

レスポンス例:

```json
{
  "mappings": {
    "10.40.0.3": "wan1"
  },
  "config": {
    "wan0": "eth0",
    "wan1": "eth1",
    "lan": "eth2"
  }
}
```

`mappings` には明示的に wan1 に切り替えた IP のみが表示されます。

## ネットワーク構成

```
LAN (eth2): 10.40.0.0/20
  ├─ デフォルト: wan0 (eth0) 経由でルーティング
  └─ 個別指定: /switch API で wan1 (eth1) に切り替え可能

例:
- 10.40.0.1 → wan0 (eth0) [デフォルト]
- 10.40.0.2 → wan0 (eth0) [デフォルト]
- 10.40.0.3 → wan1 (eth1) [API で切り替え後]
- 10.40.0.4 → wan0 (eth0) [デフォルト]
```

## 注意事項

- このツールは `ip` コマンドで system network state を変更するため、注意深く使用してください
- 必ず root 権限で実行してください
- LAN サブネットは `10.40.0.0/20` 固定です（コード内で定義）
