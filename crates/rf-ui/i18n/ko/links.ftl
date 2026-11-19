# 링크 (OPS-08): 링크 만들기 ▸의 종류, 알림, 새 바로 가기의 이름.

## 링크의 종류. 바로 가기는 macOS에서 별칭이다.

link-kind-shortcut = 바로 가기
link-kind-alias = 별칭
link-kind-symlink = 심볼릭 링크
link-kind-junction = 정션
# 꺼진 종류: 메뉴 항목에는 툴팁이 없어 까닭을 이름 뒤에 쓴다. $name: 종류(link-kind-*).
link-entry-off = { $name } (개발자 모드나 관리자 권한 필요)
link-needs-privilege = 심볼릭 링크를 만들려면 Windows 설정에서 개발자 모드를 켜거나 관리자 권한으로 실행해야 합니다

## 알림

links-made = 링크 { $count }개를 만들었습니다
# 알림 뒤에 " · "로 붙는다.
links-dangling = 대상이 없는 링크 { $count }개
link-refused = 링크를 만들지 못했습니다: { $reason }

## 새 바로 가기의 이름에서 대상의 이름 뒤에 붙는 말
#
# Windows는 "보고서.docx - 바로 가기.lnk", macOS는 "사진 별칭"으로 만든다(꼴은 rf_platform::link_name).

link-name-shortcut = 바로 가기
link-name-alias = 별칭
