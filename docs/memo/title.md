# Title

Xtermにおけるターミナルのタイトル更新

## OSC 

Format: `OSC Ps ; Pt {BEL|ST}`

Psは変更先の指定、Ptはテキスト

| Ps | Description | 
| - | - |
| 0 | Change Icon Name and Window Title |
| 1 | Change Icon Name | 
| 2 | Change Window Title | 
| 4 6 | Change Log File to Pt.  This is normally disabled by a compile-time option | 