# Penplot-Gcoder Studio

SVG・DXF・画像を読み込み、文字や手描きを加え、3D プリンター用のペン描画経路を編集・検査して `.GCODE` に保存するオフラインの Rust デスクトップアプリ。Windows 向け GUI と、GUI なしで同じファイルを生成できる CLI がある。**Python・クラウド・プリンターへの接続は不要**。

![実際の Studio の 2D プレビュー。実線は描画・オレンジ破線は移動経路](docs/studio-preview.png)

## まず試す（実機なし）

1. Windows x64 ビルドの `penplot-studio.exe` を起動。ソースからは下記 `cargo run`。同梱の `examples/alignment-card.svg` を画面にドロップする。
2. 2D の **Move / Scale** は同じモードで、枠内をドラッグして移動、角をドラッグして縦横比を保って拡大・縮小。表示中の全レイヤーに作用する。回転は **Rotate** の丸いハンドルをドラッグ。Shift で移動を軸に固定・回転を 15° 刻みにし、Esc で操作を取り消せる。**Edit** の左側で配置値を、右側でレイヤー順を編集できる。**G-code Preview** の左側の **PATH PLANNING** で輪郭だけ／平行ハッチ／クロスハッチを比較する。実線が描画。ペン上げ移動の破線は タイムラインの Show travel で表示する（初期状態は非表示）。タイムラインでストローク順を追える。**3D lifts** は機械設定の上下 Z とストローク間のリフトを固定視点で立体表示し、Z は 1〜12 倍に誇張できる（[画面例](docs/studio-lifts.png)）。実機の干渉判定ではない。
3. 外枠は原寸 40 × 24 mm。**Fit** は余白を残して拡縮するので、原寸のまま使うなら Scale = 1。図形は回転→拡縮し、回転後外接枠の左下を紙面 X/Y に合わせる。
4. ファイルを実機で使う段階で、**G-code Preview → MACHINE PROFILE** に接触・退避 Z、送り、実際の可動範囲と取付補正を入力する。先に **Export air run…** で全経路ペン上げの空走ファイルを出力する。[K9 の実測・手動 HOME・初回テスト](docs/K9.md) / [P1S・Aries の機種プリセットと絶対 Z](docs/MACHINE-PRESETS.md)

書き出し後は、保存した G-code の丸め済みコマンドから作った 2D プレビューを **G-code Preview** タブで表示します。編集中のルートとは独立したスナップショットで、通常描画のペン上げ移動は表示を切り替えられます。画面下部の概算時間は通常／空走を併記し、加減速・初期移動・ファームウェアの処理時間を含みません。

## ワークスペース（v0.5.0）

**実画面（v0.5.0、Linux 版）**

![編集タブ：左にツール、中央に描画面、右にレイヤー](docs/screenshots/studio-v0.5.0-edit.png)

![G-code Preview タブ：K9 の機械設定、ペンの経路、生成コマンド](docs/screenshots/studio-v0.5.0-gcode-preview.png)

- **Edit**: 左にツールボックスとツールオプション、中央にキャンバス、右にレイヤー。Move / Scale は統合のまま、回転・図形・文字・Delete・右クリックを使用できます。
- **Import**: ファイルの取り込みと画像の Threshold / Outline / Centerline / Fill を調整します。取り込み途中でもタブを往復でき、設定と最後の有効なプレビューを保持します。Add image で確定、Cancel でその画像を取り消します。
- **G-code Preview**: 左に機械・経路設定、中央に出力コマンドのプレビュー、右にレイヤー。既存のスナップショットは自動で置き換えず、更新後は「以前のコマンド」と表示します。Refresh from current job で現在の結果を確認できます。
- 上部の **Export** は全タブ共通。再計算中は最新結果を待ち、無効な機械設定や描画範囲では保存しません。未確定の画像・文字は確定またはキャンセルが必要です。
- タブ変更では編集内容、選択、カメラ位置と倍率、取り込み設定、Undo/Redo、生成済みプレビューを保持します。760 × 560 以上の画面で左右パネルが幅を抑え、各パネル内をスクロールできます。

### ソースからビルド

Rustup とプラットフォームの C/C++ リンカーを準備する（Windows は Rust MSVC + Visual Studio Build Tools の C++ ワークロード）。リポジトリの `rust-toolchain.toml` が Rust 1.90.0 を選択し、`Cargo.lock` が依存関係を固定する。初回ビルドは依存関係をダウンロードする。実行時のネット接続は不要。

```sh
cargo run --locked --release --bin penplot-studio
cargo build --locked --release --bins
# ヘッドレス：egui / ウィンドウシステムなし
cargo build --locked --release --no-default-features --bin penplot
```

Windows 出力は `target/release/penplot-studio.exe` と `penplot.exe`。Linux GUI は X11 または Wayland と OpenGL を使用。Linux のネイティブファイルダイアログにはデスクトップポータルが必要。GitHub Actions の **Rust workstation** は Windows と Linux を検査し、成功した Windows ジョブの Artifacts に exe を置く（Release の自動公開はしない）。

## 入力と編集

| 入力 | Rust 版での扱い |
|---|---|
| SVG | viewBox・物理単位・グループ変換・ベジェ/円弧を展開。1 CSS px = 25.4/96 mm。独立したサブパスを結ばない。テキストはパスへ、画像・クリップ・マスク・フィルタ等は事前に輪郭化。省略の警告を確認する。|
| ASCII DXF | モデル空間の LINE、ARC、CIRCLE、ELLIPSE、2D POLYLINE／LWPOLYLINE（bulge）、2D SPLINE（制御点・ノットを持つ 1〜8 次の多項式／有理曲線）。フィット点だけのスプラインは対象外。$INSUNITS を mm に換算し、未指定時は mm として警告。バイナリ DXF、INSERT、3D/メッシュは対象外。|
| PNG / JPEG | DPI と輝度しきい値（反転可能）で、既定の **Outline** は画素セルの外周と穴を、**Centerline** は暗線を細線化して中心の経路だけを抽出。透明部分は白、図の Y 軸を上向きに変換。画像の EXIF 回転・DPI メタデータは自動適用しない。|

- ファイルごとのレイヤー表示・順序・削除、線／矩形／楕円／フリーハンド、Undo/Redo。複数ファイルは元座標で重ねて配置する。
- **Project → Save / Open** で複数レイヤー、描き加えた図形、配置・描画条件と機械設定を単一の `.penplot` に保存する。元画像へのリンクではなく抽出後の形状を格納し、ファイルの再配置や別 PC でも再開できる。有効な計算済み経路があれば通常／空走を書き出せる。範囲外・空経路・計算中の古い経路は書き出さない。
- 画像の取り込み画面では Threshold、Image DPI、反転、輪郭のみ／ハッチ／クロスハッチを画像ごとに指定。しきい値やトレース方式の計算が終わるまでは直前の経路を表示し、古いプレビューを確定しない。塗り方・間隔・角度は取り込み後もレイヤーごとに変更でき、方式・しきい値・DPI・反転の変更には元画像を再度取り込む。**Centerline** では輪郭やハッチを重ねない。分岐では線をなぞり直さず必要に応じてペンを上げる。極小点・孤立点や太い面の意図した筆順は復元できない。CLI は `--trace centerline`（画像のみ）。
- 左パネル上部の **Min image stroke** は最終配置後の画像経路だけを指定長未満で省く。数値をクリックして mm を入力でき、**Off** で 0（無効）に戻す。輪郭の途中は切らない。プレビュー・通常出力・空走に共通。CLI では `--min-image-stroke MM`。
- ハッチは閉じた塗り領域に限定し、同一グループの穴を偶奇則で除く。近い線を勝手につながない。最適化は同じレイヤーの連続区間で並び替え・開線の逆転を試みる。
- 入力は 16 MiB まで（ベクターは 8 MiB）、画像は 8 MP、ドキュメント全体は 50 万頂点・10 万経路まで。さらに曲線／ハッチ密度による処理上限あり。形状が重すぎる場合は簡略化するかハッチの間隔を広げる。
- **紙面と機械の両方で範囲を検証**。問題があれば保存をブロックする。出力は mm・絶対座標、送り F は mm/min。発熱・押出・自動 HOME・モーター解放は行わない。

## 文字を追加・編集する

**Text…** を開き、本文、Family（書体ファミリ）、Face（コレクション内のフェイス）、Em size（mm）と **Trace（Outline / Centerline）** を選び、**Preview paths → Add text** で独立した文字レイヤーを追加する。本文は日本語・英数混在を含む横書きの複数行に対応する。Anchor X/Y は紙面上の位置。Em size は字面の実測高さではなくフォントの em サイズで、ジョブの拡縮は追加済みの文字にも作用する。

追加後はレイヤー選択後の **Edit text…**、または文字上／レイヤー行の右クリックから再編集できる。変更はプレビュー後の **Apply text** まで確定されず、キャンセル・生成エラーは元の形状を保持する。削除と Undo/Redo は文字レイヤー単位。Outline は文字の穴も閉輪郭として描画する。Centerline は同じフォントの字形から推定する一本線で、輪郭・塗りつぶしを重ねない。O のような形では通常ひとつのループになる。細い部分やセリフに短い枝が出ることがあるのでプレビューで確認する。フォントに筆順データが加わるわけではない。

- Windows のマシン全体とユーザー別にインストールされたフォントをオフラインで列挙する。**Font file…** から手元の TTF / OTF / TTC / OTC も指定できる。ファイルは 64 MiB、コレクションは 256 フェイスまで。非インストールのフォントは再編集時に同じファイルを選び直す場合がある。
- OpenType のシェーピング、結合文字、対応するリガチャ、双方向のスクリプト処理を行う。必要な文字がない場合はコードポイントを含むエラーを表示する。自動フォールバックで別フォントに変えることはない。
- 可変フォントは既定軸。軸の調整、縦書き、任意の named instance 選択は現版では未対応。ビットマップ専用や描画用の輪郭を持たないカラーグリフは使えない。すべてのフォント／字形を描ける保証ではない。
- `.penplot` には本文・フォントの識別子・保存済み経路を記録し、フォント本体やローカルパスは含めない。フォントがない別 PC でも保存済みの絵と G-code を再現できる。再編集時は元と同じバイト／フェイスを照合し、違うフォントでは新しい経路をプレビューして **Use this replacement font** を選ぶまで適用できない。
- 本文は 2,000 Unicode 文字／100 行。プロジェクト全体の 16 MiB・50 万頂点・10 万経路の制限も適用する。文字を含む保存ファイルは形式 v3、文字なしは v2。既存の v1/v2 プロジェクトを開ける。旧形式の輪郭設定を継承する。中心線の文字メタデータを含む保存ファイルは v0.4.0 以降で開く。

Linux の実フォントとヘッドレス egui の Unicode／IME イベントで検証している。ネイティブ Windows のフォント列挙・実際の IME 操作と K9 実機動作は未検証。

## CLI

```sh
# 機械に触れずにプリフライト。変換結果をファイルに保存しない
penplot check --x 5 --y 5 examples/alignment-card.svg

# 機種名付きの編集用ひな型を書き出す（値は未校正）
penplot profile-init measured.json --k9
# ここで measured.json を実機の測定値に合わせる

# 手動で HOME し、可動域・Z・オフセットを実測した後のみ
penplot convert --profile measured.json --calibrated --air-run --x 5 --y 5 -o AIRRUN.GCODE examples/alignment-card.svg
penplot convert --profile measured.json --calibrated --x 5 --y 5 -o PLOT.GCODE examples/alignment-card.svg

# DXF 回転＋余白、SVG クロスハッチ等を実機なしで比較
penplot check --rotate 90 --fit 5 examples/calibration.dxf
penplot check --hatch cross --spacing 1 --angle 45 --optimize examples/alignment-card.svg

# Studio で保存した編集結果を、同じ設定・可視レイヤーで変換
penplot check my-job.penplot
penplot convert --calibrated --air-run -o JOBAIR.GCODE my-job.penplot
penplot --help
```

CLI は既存ファイルを上書きしない。エラー時に G-code を部分保存しない。GUI も CLI も同じライブラリで変換し、同じ条件なら出力バイト列は同じ（時刻や入力パスを埋め込まない）。

## 旧版から移行する

`profiles/printers/*.json` の同梱 4 機種は **Load JSON…** または `penplot profile-migrate profiles/printers/K9.json measured.json` で読み込める。**サイズ・原点・XY オフセットのみ**を引き継ぎ、Z・速度・生の G-code テンプレート・ファーム指定・経路設定は流用しない。旧 K9 の空のペン指令、同値の昇降 Z、0.1 mm/s、Klipper 指定を有効な純正機の設定として扱わない。Rust 版の保存形式はバージョン付き JSON。

旧 Python 版の `main.py`・`src/`・既存プロファイルは手を加えず残している。旧 `setup.bat` は **Python 版**を起動する。Rust 版で未移植なのは3D ビューの自由回転（固定等角投影は対応）、2-opt、ドット・スパイラル等の特殊塗り、自由な開始／終了テンプレート、色別ペン交換の機械命令など。現在は **Z 軸リフト専用・ファイル出力のみ**で、FAN/サーボ・USB ストリーム・K9 実機動作は未検証。

## 開発と検証

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo test --locked --no-default-features
cargo build --locked --release --bins
```

Rust の入口は `src-rs/lib.rs`（変換）、`src-rs/desktop.rs`（GUI）、`src-rs/cli.rs`（CLI）。`tests/pipeline.rs` は実ファイル、穴付き塗り、配置、空走、上書き拒否、同一出力を検査する。

### Selecting and deleting objects

Click a line in Pan, Move / Scale or Rotate to select its object, or select a row in the Layers list. Imported SVG/DXF/images are selected as whole layers; a canvas mark is selected as one continuous stroke. A layer row always targets that whole layer. Delete or the scoped right-click menu removes the selected object, and Ctrl+Z restores it (Ctrl+Y / Ctrl+Shift+Z redo). Right-clicking blank canvas clears the selection. Text/number editing, dialogs, import work and active drags keep their own keys. Deleting an object preserves the placement of the remaining artwork and invalidates generated output until replanning finishes.
