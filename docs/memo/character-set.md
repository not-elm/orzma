# Character Set

ここでは、図形文字集合をG0〜G3へ指示し、それらをGLまたはGRへ呼び出すことで、受信した符号値と表示する図形文字との対応を切り替える機能に焦点を当てて文書化する。

現代のターミナルエミュレータで広く実装されているのは、主に94文字集合のASCIIおよびDEC Special Graphicsである。そのため、Orzmaが扱う図形文字集合は94文字集合に限定し、96文字集合は実装対象外とする。

基本的には[VT220 Programmer Reference Manual](https://vt100.net/docs/vt220-rm)(以下マニュアル)の以下の章から必要な情報だけまとめる。
VT220を参照している理由は現状の最新(2026-08-22)はVT510だが、VT220のマニュアルの方が詳細に記述されているため。
- [2.4 Character Sets](https://vt100.net/docs/vt220-rm/chapter2.html#S2.4)
- [4.4 Character Set Selection (SCS)](https://vt100.net/docs/vt220-rm/chapter4.html#S4.4)

VT510で追加された文字集合とSCSについては、[VT510 Video Terminal Programmer Information](../references/vt510.pdf)のChapter 5およびChapter 7を参照する。

## ドメイン

### GL/GR

コードテーブルにはC0/C1のような制御文字用の領域のほかに、図形文字の符号位置として使用される領域が存在する。
0x20–0x7Fの範囲をGL、0xA0–0xFFをGRと呼称する。

### 文字集合 - Character Sets

図形文字集合は、符号位置と、その位置で表示する図形文字との対応を定義した集合である。
同じ符号値でも、選択された図形文字集合によって表示する文字は異なる。例えば`0x71`は、ASCII graphicsでは`q`、DEC Special Graphicsでは横罫線として表示される。

VT端末が利用できる図形文字集合の総体をgraphic repertoireと呼ぶ。図形文字集合は、G符号への指示とGLまたはGRへの呼出しを経て、受信した符号値の解釈に利用される。

図形文字集合には、主に次の2種類がある。

| 種類 | 符号位置 | 説明 |
| - | - | - |
| 94文字集合 | GLでは`0x21`–`0x7E`、GRでは`0xA1`–`0xFE` | GLの`0x20`と`0x7F`、GRの`0xA0`と`0xFF`を集合に含めない。 |
| 96文字集合 | GLでは`0x20`–`0x7F`、GRでは`0xA0`–`0xFF` | G1、G2、G3へ指示できる。G0へは指示できない。 |

VT510の8-bit multinational character setは、ASCIIを左半分のGL、対応するsupplemental setを右半分のGRとして組み合わせたものである。2つの図形文字集合は個別に指示・呼出しできるが、通常は1つの8-bit文字集合として組み合わせて使用する。

VT510は次のVT用文字集合をサポートする。

| 分類 | 文字集合 |
| - | - |
| 8-bit multinational | ISO Latin-1、ISO Latin-2、ISO Latin-Cyrillic、ISO Latin-Greek、ISO Latin-Hebrew、ISO Latin-5、KOI-8 Cyrillic、DEC Multinational、DEC Greek、DEC Hebrew、DEC Turkish |
| その他のVT用図形文字集合 | DEC Special Graphics、DEC Technical Character Set |
| 7-bit NRCS | U.K.、French、DEC French Canadian、Norwegian/Danish、DEC Finnish、German、Italian、DEC Swiss、Swedish、Spanish、DEC Portuguese、SCS、Russian（KOI-7）、DEC 7-bit Greek、DEC 7-bit Hebrew、DEC 7-bit Turkish |
| DRCS | Down-line-loadableなユーザー定義文字集合。最大2組の96文字を保持できる。 |

Orzmaでは一旦以下の集合だけサポートする。
- ASCII
- DEC Special Graphics（罫線など）

### G Sets

G SetsはG0, G1, G2, G3の４つから構成される。１要素を示す正式な名称はおそらく存在しないが、このドキュメントでは`G符号(G code)`と呼称する。
各G符号は、指示された図形文字集合を参照する。どのG符号をGLまたはGRへ呼び出しているかは、G符号とは別の状態として保持する。

[Figure4-1](https://vt100.net/docs/vt220-rm/chapter4.html#F4-1)

## 図形文字のマッピングの流れ

1. SCSにより、図形文字集合をG0〜G3のいずれかのG符号へ指示する。Orzmaでは94文字集合のみを扱う。
2. Locking Shiftにより、G符号をGLまたはGRへ呼び出す。Single Shiftを受信した場合は、次に受信する図形文字1文字に限り、G2またはG3をGLへ一時的に呼び出す。
3. 受信した符号値がGLとGRのどちらに属するかを判定し、その領域へ呼び出されているG符号と集合内の符号位置を特定する。Single Shiftが有効な場合は、GLの通常の呼出し状態よりもSingle Shiftを優先する。
4. G符号へ指示されている図形文字集合を参照し、符号位置に対応する図形文字を描画する。

例えば、`ESC ( 0`はDEC Special GraphicsをG0へ指示する。LS0によってG0がGLへ呼び出されている状態で`0x71`を受信すると、DEC Special Graphicsにおける`0x71`の図形文字である横罫線を描画する。

## 制御関数一覧

### SCS

指定のG符号に対して、使用する図形文字集合を指示する。

> ESC {G_CODE} {Dscs}

`{G_CODE}`では、図形文字集合の大きさと指示先のG符号を指定する。

| 文字集合 | code | G符号 |
| - | - | - |
| 94文字集合 | `(` | G0 |
| 94文字集合 | `)` | G1 |
| 94文字集合 | `*` | G2 |
| 94文字集合 | `+` | G3 |
| 96文字集合 | `-` | G1 |
| 96文字集合 | `.` | G2 |
| 96文字集合 | `/` | G3 |

`Dscs`は、指示する図形文字集合の識別子である。`B`のような1文字だけでなく、`%5`や`&4`のようにintermediate characterを含む場合がある。

#### VT220の94文字集合

| Dscs | 文字集合 |
| - | - |
| `B` | ASCII |
| `<` | DEC supplemental（VT200 modeのみ） |
| `0` | DEC special graphics |
| `A` | British NRC |
| `4` | Dutch NRC |
| `C` or `5` | Finnish NRC |
| `R` | French NRC |
| `Q` | French Canadian NRC |
| `K` | German NRC |
| `Y` | Italian NRC |
| `E` or `6` | Norwegian/Danish NRC |
| `Z` | Spanish NRC |
| `H` or `7` | Swedish NRC |
| `=` | Swiss NRC |

#### VT510の94文字集合

| Dscs | 文字集合 |
| - | - |
| `B` | ASCII |
| `%5` | DEC Supplemental |
| `"?` | DEC Greek |
| `"4` | DEC Hebrew |
| `%0` | DEC Turkish |
| `&4` | DEC Cyrillic |
| `A` | U.K. NRCS |
| `R` | French NRCS |
| `9` or `Q` | French Canadian NRCS |
| `` ` ``, `E`, or `6` | Norwegian/Danish NRCS |
| `5` or `C` | Finnish NRCS |
| `K` | German NRCS |
| `Y` | Italian NRCS |
| `=` | Swiss NRCS |
| `7` or `H` | Swedish NRCS |
| `Z` | Spanish NRCS |
| `%6` | Portuguese NRCS |
| `">` | Greek NRCS |
| `%=` | Hebrew NRCS |
| `%2` | Turkish NRCS |
| `%3` | SCS NRCS |
| `&5` | Russian NRCS |
| `0` | DEC Special Graphics |
| `>` | DEC Technical Character Set |
| `<` | User-preferred Supplemental |

#### VT510の96文字集合

| Dscs | 文字集合 |
| - | - |
| `A` | ISO Latin-1 Supplemental |
| `B` | ISO Latin-2 Supplemental |
| `F` | ISO Greek Supplemental |
| `H` | ISO Hebrew Supplemental |
| `M` | ISO Latin-5 Supplemental |
| `L` | ISO Latin-Cyrillic |
| `<` | User-preferred Supplemental |

VT220では`<`をDEC Supplementalの識別子として使用する。一方、VT510では`<`はUser-preferred Supplementalを表し、その初期値がDEC Supplementalである。したがって、同じ識別子でも端末の互換レベルとuser-preferred setの設定によって実際の文字集合が異なる場合がある。

### Locking Shift

G符号をGLまたはGRへ呼び出す。呼出し状態は、同じ領域に対して別のlocking shiftを受信するまで継続する。
LS0とLS1は、それぞれ従来の呼称であるSI（Shift In）とSO（Shift Out）でも表される。

| 制御関数 | 符号表現 | バイト列 | G符号 | 呼出し先 |
| - | - | - | - | - |
| LS0 | `SI` | `0F` | G0 | GL |
| LS1 | `SO` | `0E` | G1 | GL |
| LS1R | `ESC ~` | `1B 7E` | G1 | GR |
| LS2 | `ESC n` | `1B 6E` | G2 | GL |
| LS2R | `ESC }` | `1B 7D` | G2 | GR |
| LS3 | `ESC o` | `1B 6F` | G3 | GL |
| LS3R | `ESC |` | `1B 7C` | G3 | GR |

LS1R、LS2、LS2R、LS3、LS3RはVT200 modeでのみ使用できる。

[Table 4-5](https://vt100.net/docs/vt220-rm/table4-5.html)

### Single Shift

次の図形文字1文字に限り、G2またはG3をGLへ一時的に呼び出す。文字を表示した後は、single shiftの前にGLへ呼び出されていたG符号へ戻る。locking shiftの呼出し状態は変更しない。

| 制御関数 | 7-bit形式 | 7-bitバイト列 | 8-bit形式 | G符号 | 呼出し先 |
| - | - | - | - | - | - |
| SS2 | `ESC N` | `1B 4E` | `8E` | G2 | GL（次の1文字のみ） |
| SS3 | `ESC O` | `1B 4F` | `8F` | G3 | GL（次の1文字のみ） |

- [SS2 — Single Shift G2](https://vt100.net/docs/vt220-rm/chapter4.html#S4.4.4.1)
- [SS3 — Single Shift G3](https://vt100.net/docs/vt220-rm/chapter4.html#S4.4.4.2)

## 現代のターミナルにおける実装状況の変化

元々GRはGLだけでは表現しきれない描画文字をサポートするためのものだったが、現代ではUTF-8を利用するのが主流となっており、更にUTF-8の先頭バイトがGRのコード範囲に重なっているという理由もあり多くのターミナルではGRは利用されなくなっている。
GLに関しては引き続き利用されており実装する必要がある。