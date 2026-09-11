# Checkpoint

メモリ上にカーソル位置などの状態を保存、復元するための機能。名前だけ見るとカーソルの情報だけ保持するように誤認するが、実際は文字集合の状態なども対象となる。           
現代ではALT-Screen/Primary-Screenを跨いでカーソルを復元するためにも使用される。

VT510の仕様では`Save Cursor`のような名称が使用されているが、実際にはカーソル以外の情報も含まれているため、Orzmaでは`Checkpoint`という名称を採用している。

保存対象となる状態の一覧は下記に記載。

| 状態 | 説明 |
| --- | --- |
| カーソル位置 | 現在の行と列を保存する。 |
| SGR文字属性 | SGRで設定された前景色、背景色、太字、下線などの文字属性を保存する。 |
| [G0–G3およびGL/GR](./character-set.md) | G0–G3に指示された文字集合と、GLおよびGRに呼び出されている文字集合を保存する。 |
| [SS2／SS3](./character-set.md) | 次の1文字に対してG2またはG3を一時的に呼び出す、未適用のsingle shift状態を保存する。 |
| Wrap flag | **DECAWM ではなく LCF（last column flag / deferred wrap）を保存する。** VT510 の「Wrap flag (autowrap or no autowrap)」という表記は自動折り返しモードそのものと読めるが、DEC STD-070 p.D-14 は「LCF を Save Cursor で保存し Restore Cursor で復元すべき」と明記し、xterm も `DECSC_FLAGS` から `WRAPAROUND` を除外した上でコメントでこの読みを逐語で却下している。実機 VT100/220/420/510 も DECRC で DECAWM を復元しない。 |
| Origin mode | カーソル位置の基準を画面全体またはスクロール領域とするDECOMの状態を保存する。 |
| Selective erase attribute | 以後に書き込む文字を選択消去の対象とするか、保護対象とするかを示す属性を保存する。 |


## 制御関数一覧

### DECSC

`ESC 7`

状態をメモリ上に保存する。命令には各種パラメータは割り当てられないため保存する状態は端末側で管理する必要がある。

### DECRC

`ESC 8`

状態をメモリから復元する。

DECSCに保存された内容がない場合、DECRCは次の処理を行う。
- カーソルをホーム位置（画面左上）へ移動
- オリジンモード（DECOM）をリセット
- すべての文字属性をオフ(通常設定)
- ASCII文字セットをGLに、DEC補助図形文字セットをGRに割り当てる
