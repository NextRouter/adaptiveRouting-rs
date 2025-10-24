# adaptiveRouting

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

## 仕組みの詳細

このシステムは **Linux のポリシーベースルーティング** と **nftables による NAT/フィルタリング** を組み合わせて、LAN 内の各 IP アドレスを異なる WAN インターフェースに動的にルーティングします。

### アーキテクチャ

```
┌─────────────────────────────────────────────────────┐
│ LAN (eth2): 10.40.0.0/20                            │
│  - 10.40.0.1, 10.40.0.2, 10.40.0.3, ...             │
└──────────────────┬──────────────────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────────────────┐
│ Router (本システム)  　                               │
│  ┌─────────────────────────────────────────────┐    │
│  │ 1. nftables (パケットフィルタ & NAT)  　       │    │
│  │    - FORWARD: LAN→WAN を許可                 │    │
│  │    - MASQUERADE: 送信元 IP を WAN IP に変換　　│    │
│  └─────────────────────────────────────────────┘    │
│  ┌─────────────────────────────────────────────┐    │
│  │ 2. ルーティングテーブル              　      　 │    │
│  │    - Table 100 (wan0): デフォルトゲートウェイ 　│    │
│  │    - Table 200 (wan1): 代替ゲートウェイ      　│    │
│  └─────────────────────────────────────────────┘    │
│  ┌─────────────────────────────────────────────┐    │
│  │ 3. IP ルール (ポリシーベースルーティング)  　　   │    │
│  │    - Priority 1000: 特定IP → Table 200  　   │    │
│  │    - Priority 2000: 10.40.0.0/20 → Table 100│    │
│  └─────────────────────────────────────────────┘    │
│  ┌─────────────────────────────────────────────┐    │
│  │ 4. HTTP API (本プログラム)                    │    │
│  │    - /switch: IP ルールを動的に追加/削除 　     │    │
│  │    - /status: 現在の状態を表示　　　　　　　　　　│    │
│  └─────────────────────────────────────────────┘    │
└──────────────┬──────────────────┬───────────────────┘
               │                  │
               ▼                  ▼
   ┌───────────────────┐  ┌───────────────────┐
   │ WAN0 (eth0)       │  │ WAN1 (eth1)       │
   │ Table 100         │  │ Table 200         │
   └───────────────────┘  └───────────────────┘
```

### 1. nftables の役割

`nftables.conf` で定義されているファイアウォールルールが以下の機能を提供します：

#### フィルタリング (inet filter テーブル)

- **input チェイン**: ルーター自身への接続を制御
  - ループバック (lo) を許可
  - SSH (eth3 から 126.0.0.0/24) を許可
  - LAN (eth2) からの接続を許可
  - その他は確立済み接続のみ許可
- **forward チェイン**: パケット転送を制御

  - LAN (eth2) → WAN (eth0/eth1) への転送を許可
  - WAN → LAN への確立済み接続の返信を許可

- **output チェイン**: ルーターからの送信を許可 (policy accept)

#### NAT (inet nat テーブル)

- **postrouting チェイン**: 送信パケットの送信元 IP を書き換え
  - `oifname "eth0" masquerade`: eth0 から出るパケットの送信元を eth0 の IP に変換
  - `oifname "eth1" masquerade`: eth1 から出るパケットの送信元を eth1 の IP に変換

**重要**: nftables は**どの WAN を使うかは決めません**。それはルーティングテーブルと IP ルールの仕事です。nftables は選択された WAN インターフェースで NAT を行い、パケットの転送を許可するだけです。

### 2. ポリシーベースルーティング

Linux カーネルは複数のルーティングテーブルをサポートしています。本システムでは以下の 2 つのテーブルを使用します：

#### ルーティングテーブル 100 (wan0 用)

```bash
ip route replace default via <wan0_gateway> dev eth0 table 100
```

- wan0 のゲートウェイをデフォルトルートとして設定
- このテーブルを使うパケットは eth0 から出ていく

#### ルーティングテーブル 200 (wan1 用)

```bash
ip route replace default via <wan1_gateway> dev eth1 table 200
```

- wan1 のゲートウェイをデフォルトルートとして設定
- このテーブルを使うパケットは eth1 から出ていく

### 3. IP ルール (どのテーブルを使うか決定)

IP ルールは「どの送信元 IP がどのルーティングテーブルを使うか」を定義します：

#### デフォルトルール (Priority 2000)

```bash
ip rule add from 10.40.0.0/20 lookup 100 priority 2000
```

- LAN サブネット全体がテーブル 100 (wan0) を使用
- 起動時に自動設定

#### 個別 IP のオーバーライド (Priority 1000)

```bash
ip rule add from 10.40.0.3/32 lookup 200 priority 1000
```

- 特定の IP だけがテーブル 200 (wan1) を使用
- Priority が小さいほど優先度が高い
- `/switch` API で動的に追加/削除

### 4. パケットフロー例

**例 1: 10.40.0.1 からインターネットへのアクセス (デフォルト)**

```
1. LAN デバイス 10.40.0.1 がパケット送信
   src: 10.40.0.1, dst: 8.8.8.8

2. ルーターに到達

3. IP ルール照合
   - Priority 2000 ルールにマッチ: from 10.40.0.0/20 → Table 100

4. Table 100 のルートを使用
   - default via <wan0_gw> dev eth0

5. nftables の forward チェイン
   - iifname "eth2" oifname "eth0" → 許可

6. nftables の NAT (postrouting)
   - oifname "eth0" masquerade
   - src: 10.40.0.1 → <eth0_ip>

7. eth0 (wan0) から送信
   src: <eth0_ip>, dst: 8.8.8.8
```

**例 2: 10.40.0.3 を wan1 に切り替え後**

```
# API 呼び出し
curl "http://localhost:32599/switch?ip=10.40.0.3&nic=wan1"

# 内部で実行されるコマンド
ip rule add from 10.40.0.3/32 lookup 200 priority 1000
```

```
1. LAN デバイス 10.40.0.3 がパケット送信
   src: 10.40.0.3, dst: 8.8.8.8

2. ルーターに到達

3. IP ルール照合
   - Priority 1000 ルールにマッチ: from 10.40.0.3/32 → Table 200
     (Priority 2000 より優先)

4. Table 200 のルートを使用
   - default via <wan1_gw> dev eth1

5. nftables の forward チェイン
   - iifname "eth2" oifname "eth1" → 許可

6. nftables の NAT (postrouting)
   - oifname "eth1" masquerade
   - src: 10.40.0.3 → <eth1_ip>

7. eth1 (wan1) から送信
   src: <eth1_ip>, dst: 8.8.8.8
```

### 5. プログラムの初期化処理

起動時に `initialize_lan_to_wan0()` 関数が以下を実行します：

1. **ゲートウェイ検出**

   ```rust
   let gw0 = get_default_gateway_for_iface(&config.wan0)
   let gw1 = get_default_gateway_for_iface(&config.wan1)
   ```

   - `ip route show default dev eth0` を実行して各 WAN のゲートウェイを取得

2. **ルーティングテーブルの設定**

   ```rust
   ensure_table_default_route(&config.wan0, TABLE_WAN0, &gw0)
   ensure_table_default_route(&config.wan1, TABLE_WAN1, &gw1)
   ```

   - Table 100 と 200 にデフォルトルートを設定

3. **リンクルートのミラーリング**

   ```rust
   mirror_link_routes_to_table(&config.wan0, TABLE_WAN0)
   mirror_link_routes_to_table(&config.wan1, TABLE_WAN1)
   ```

   - 各 WAN の直接接続されたネットワーク (scope link) を各テーブルにコピー
   - ARP 解決とゲートウェイへの到達性を確保

4. **デフォルトポリシールールの設定**
   ```rust
   add_ip_rule(lan_subnet, TABLE_WAN0, PRIO_LAN_DEFAULT)
   ```
   - LAN サブネット全体を wan0 にルーティングする基本ルールを追加

### まとめ

- **nftables**: パケットフィルタリングと NAT を担当。ルーティングの決定はしない
- **ルーティングテーブル**: 各 WAN へのルートを保持 (Table 100 = wan0, Table 200 = wan1)
- **IP ルール**: 送信元 IP に基づいてどのテーブルを使うか決定
- **本プログラム**: HTTP API で IP ルールを動的に管理し、特定 IP だけを wan1 に切り替える

この設計により、LAN 内の各デバイスを個別に異なる WAN に割り当てることができ、負荷分散やフェイルオーバーが柔軟に実現できます。

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
sudo ./target/release/adaptiverouting
```

デフォルト設定:

- wan0: eth0
- wan1: eth1
- lan: eth2

### サーバー起動（環境変数で設定）

```sh
# カスタムインターフェース名を指定
sudo WAN0=enp0s3 WAN1=enp0s4 LAN=enp0s5 ./target/release/adaptiverouting
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
