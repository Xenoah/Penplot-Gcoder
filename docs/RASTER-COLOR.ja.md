# 線画抽出と色別ハッチ（v0.10.0）

画像の Import 画面で **Threshold (classic)** / **Clean line art** / **Color hatch** を選びます。従来の Outline / Centerline は Threshold の中に残っています。PNG と JPEG が対象で、SVG/DXF の取り込み方式は変更しません。

## Clean line art

局所的な明るさを背景の目安にして線を選び、小さな連結成分を除去してから中心線に変換します。細線を消す収縮処理や線間をつなぐ膨張処理は使いません。

- **Background radius (px)**: 背景を推定する範囲。太い線には大きめにします。元画像の画素単位です。
- **Ink contrast**: 周囲との差。下げると薄い線を拾いやすく、上げると背景の凹凸を抑えます。
- **Remove dots ≤ px**: この画素数以下の点を除去。0 で無効。短い句読点なども除去対象になるのでプレビューで調整します。
- **Image DPI**: 描画する実寸を指定します。背景推定の画素範囲は変わりません。

調整中は直前の有効プレビューを表示し、新しい結果ができるまで Add image を無効にします。背景が激しく変化する写真、広いベタ面、接した線、意図した筆順の復元には限界があります。元画像は保存しないため、取り込み後に抽出条件を変える場合は再取り込みしてください。

同梱の `examples/clean-scan.png` は、明るさが150〜229に変化する背景上の1画素の薄い T 字と点ノイズです。Clean line art の既定値で T 字の3枝を残し、2画素の点を除去します。画像に私有素材は含みません。

## Color hatch

画像から最大2〜8色の小さなパレットを推定し、各色を**別の編集可能なレイヤー**にします。使用するペンは表示された RGB に近いものを選んでください。透明画素を描かず、半透明は白に合成して扱います。

- **Exclude white background / White cutoff**: RGB の全成分が指定値以上の画素を除外。白以外の任意背景を自動識別する機能ではありません。
- **Hatch fill / Cross-hatch**: 平行線または交差する2方向の線を生成します。
- **Spacing (mm) / Angle (deg)**: 紙上の間隔と角度。取り込み後も右の IMAGE LAYER FILL から色ごとに変更できます。レイヤーを拡大しても間隔は紙上 mm のままです。
- **Draw outlines**: 色モードは既定オフ。ハッチだけなら隣接色の共有境界を輪郭として重ね描きしません。オンでは各色の輪郭を描くため、共有境界を重ねる場合があります。

ハッチは色の画素境界と穴でクリップします。ペン先の実際の太さや紙へのにじみは考慮しません。細い領域に対して間隔が広すぎるとハッチが入らない場合があります。Color hatch は線の中心線化を行いません。一本線の線画が必要なら Clean line art を別途使ってください。

`examples/color-card.png` は、赤と青の隣接面、境界をまたぐ黒の T 字、透明な穴、黒い点を含む合成画像です。DPI 25.4、間隔 0.5 mm で試せます。

## 単ペン機で色を分けて描く

1. 全色を表示して位置とサイズを調整します。色を隠しても他色の紙座標は変わりません。全非表示を挟んで表示し直す場合も同じです。
2. **Air run** をオンにして **1. Generate G-code → 2. Preview → Export colors…**。新しいフォルダー名を指定します。既存フォルダーへの上書きはしません。
3. `P01.GCODE` などの色別ファイルと `PENS.txt` ができます。ファイル名は短いASCII名、一覧に色・順序と交換方法を記載します。元の全色配置を決めた**後に**分割するため、各色を個別に中央揃えしません。
4. 実測済みの機械設定とペンを外した空走で確認後、通常描画を別フォルダーへ出力します。一つのファイルの実行完了後、ペンが上がって停止した状態で交換します。紙と XY 原点を固定し、ペン先位置・接触 Z・退避 Z をそろえてください。

交換用 M0/M6/T 指令や自動 HOME は追加しません。G-code 内の `; INK #RRGGBB` はプレビュー用コメントで、ペン交換を実行しません。通常の **3. Export** は全色を1本のペンで連続して描くファイルです。複数色には **Export colors** を使ってください。K9 の既存の開始設定・速度・待機・垂直リフト・空走曲線は、各色にもそのまま適用します。

## CLI と保存互換

```sh
penplot check --raster clean --ink-contrast 12 --speckle 3 examples/clean-scan.png
penplot check --raster color --palette 3 --dpi 25.4 --spacing 0.5 examples/color-card.png
# measured.json は実機の値に編集済みであること
penplot convert --profile measured.json --calibrated --air-run --raster color --palette 3 --fit 5 --spacing 0.5 --colors-dir AIR-COLORS examples/color-card.png
penplot convert --calibrated --colors-dir PLOT-COLORS saved.penplot
# GUI をファイル指定で起動（ネイティブファイル選択ダイアログなしでも取り込み可）
penplot-studio examples/color-card.png
penplot-studio saved.penplot
```

新設定・輪郭なしハッチを含むプロジェクトは形式 v4、従来の文書は引き続き v2/v3 で保存します。v1〜v3 の読み込みと K9 設定を維持します。v4 のファイルには v0.10.0 以降が必要です。保存するのは抽出済みの色別形状とハッチ条件です。

## 検証範囲

合成画像で背景ムラ、1画素の細線、点ノイズ、透明、白背景除外、穴、隣接色、共有境界の輪郭省略を検査。保存・再読込・全非表示・表示順変更後の各色の紙 XY と、実際に出力した G-code の描画線を比較しています。旧 Outline/Centerline、選択レイヤーの移動・拡縮、文字、K9 の既存試験も継続します。

Linux 実画面で Clean line art の3枝と、Color hatch の3色レイヤー・穴・表示切り替えを確認しました。デスクトップポータルがないためネイティブの選択・保存ダイアログは確認できず、ファイル指定起動と CLI の実ファイル出力で検証しました。当該版の実画面は画像ファイルとして取得できず未添付です。

実機への送信・ペン交換・にじみ・位置合わせは未検証です。Windows の実 GUI 操作は未検証です。Linux のファイル選択・保存ダイアログにはデスクトップポータルが必要です。
