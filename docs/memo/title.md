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
| 21| Change Window Title to Pt (DECSWT), VT520. | 

### Icon Nameについて

>  古いX Window Systemでは、ウィンドウを最小化するとデスクトップ上
  にアイコンとして表示され、その横や下に名前が表示されました。
  ただし現代のデスクトップでは、最小化されたウィンドウに個別の名前
  を表示しないことが多いため、Icon Nameは実質的に使われなくなって
  います。ターミナルエミュレータによっては、OSC 1を無視したり
  Window Titleと同じものとして扱ったりします。