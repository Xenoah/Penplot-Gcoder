# 機種プリセットと原点からの Z / Machine presets and absolute Z

Studio の **G-code Preview → MACHINE PROFILE → Load a machine preset…** から選択します。選び直すと実測値は初期値に戻るため、調整後は JSON またはプロジェクトに保存してください。図面とレイヤーを作り直す必要はありません。

| 機種 | 公称造形範囲 (X × Y × Z) | Penplot Studio |
| --- | --- | --- |
| EasyThreed K9 | このアプリの初期設定は 100 × 100 × 10 mm | 従来と同じ。[K9 ガイド](K9.md)を参照。 |
| Bambu Lab P1S | 256 × 256 × 256 mm | **レイアウトのみ**。G-code のプレビュー・コピー・出力を停止。|
| Voxelab Aries | 200 × 200 × 200 mm | **試験的 .gcode 出力**。実測した Z を設定してから出力。実機・純正ファームウェアでの実行は未検証。 |

造形範囲はペン付きの安全な作業範囲ではありません。原点の位置・方向、治具やケーブル、ベッドの固定具などを確認し、実用範囲を設定してください。P1S と Aries の 0 mm / 0 mm は「Z 未設定」であって、指令値ではありません。

## 描画 Z と退避 Z

- **Draw Z（描画 Z）** / JSON の `pen_down_z`: ペン先が紙に軽く触れる位置。単位は mm、基準は原点復帰後の機械が使う絶対座標の Z=0。
- **Clearance Z（退避 Z）** / `pen_up_z`: 紙面・固定具と干渉しない移動高さ。**同じ Z=0 からの絶対位置**であり、描画 Z からの持ち上げ量ではありません。Z が大きいほど紙面から離れる構成のみ対応。
- **Common Z offset（共通 Z 補正）** / `offset_z`: 両方に加算されます。すでに実際の指令値を入力するなら 0 mm のままにしてください。たとえば設定上の描画 Z=2.0 mm、退避 Z=6.0 mm、補正=+0.5 mm なら、指令は Z2.500 と Z6.500（差は 4.0 mm）。数字は計算例であり機種の推奨値ではありません。
- 0.001 mm への丸め後も退避は描画より高く、両方とも 0〜Maximum Z の範囲内に設定します。Air run は描画 Z への指令を出しませんが、**最初の退避 Z への移動が安全とは保証しません**。

## Voxelab Aries

[Voxelab 公式説明書](https://enfss.voxelab3dp.com/10000/software/704d44cbccfc0bc0ab8dfa2747925e31.pdf) に 200 mm 角の造形サイズ、USB メモリーからの印刷、およびスライサー出力の `.gx` / `.g` / `.gcode` が記載されています。このアプリは **拡張子 `.gcode` のテキストのみ**。USB 制御や `.gx` の作成はしません。説明書の対応ファイル形式は、加熱・原点復帰を含まない本アプリのファイルが、個々のファームウェアで正しく動く保証ではありません。

冷えた状態でペンを外し、メーカー指定手順で安全に原点復帰できることと、開始から終了までの移動・絶対座標・ファイル認識を確認してください。小さな図形をベッド中央に配置し、Air run を先に確認。初回は本体側で停止できる状態で立会い、想定外の加熱・自動原点復帰・待機・移動があれば中止してください。認識しない場合は拡張子の付け替えや既存の印刷スタートコードの流用はしないでください。アプリの命令は G21、G90、G1 のみ（Aries では G4 待機も出力しません）。ファームがファイル外で行う動作は制御できません。

## Bambu Lab P1S

[Bambu Lab 公式クイックスタート](https://cdn1.bambulab.com/documentation/quick-start-59b0cefdc0fc4/P1S/English%20version-Quick%20Start%20Guide%20for%20P1S.pdf) の 256 mm 角は公称値です。[公式 Bambu Studio 機種設定](https://github.com/bambulab/BambuStudio/blob/master/resources/profiles/BBL/machine/Bambu%20Lab%20P1S%200.4%20nozzle.json) には除外領域があり、[通常の開始テンプレート](https://github.com/bambulab/BambuStudio/blob/master/resources/profiles/BBL/machine/Bambu%20Lab%20P1S%200.4%20nozzle%20template%20machine_start_gcode.json) はノズル・ベッド加熱、ホーミング、清掃、押出を含みます。そのテンプレートは取り込みません。ペンを付けた状態での安全な冷間原点復帰、実際に使える座標範囲、ファームウェアが受け付ける安全なジョブ形式が未検証のため、レイアウトの検討と保存のみです。単なる拡張子変更での実行や制限の回避は行わないでください。

## CLI

```text
penplot profile-init p1s.json --p1s
penplot profile-init aries.json --aries
penplot check --profile p1s.json example.svg
penplot check --profile aries.json example.svg
```

`profile-init` は出発点となる JSON を生成します。P1S、および Z が未設定の Aries での `check` は XY のレイアウト確認のみ。Aries の `convert` には実測設定、`--calibrated`、出力先が必要です。P1S は `--calibrated` や `--air-run` をつけてもファイルを書き出しません。

---

Both height fields are absolute millimetres from the same active/homed Z=0. Keep the common offset at zero if you enter final commanded heights. P1S is layout-only; Aries is experimental and starts with unmeasured Z; the K9 preset is unchanged. The app writes files only and does not connect to, home, heat or control any printer.
