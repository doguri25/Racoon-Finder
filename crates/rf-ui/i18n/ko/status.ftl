# 패널의 상태 줄 (NAV-12): 고른 것, 항목 수, 거르기, 남은 공간. 조각을 " · "로 잇는다.

status-selected = { $count }개 선택
# $size: 고른 파일의 크기를 더한 것.
status-size = 크기 { $size }
# 고른 것이 폴더뿐일 때.
status-folders = 폴더 { $count }개
# 파일과 함께 고른 폴더.
status-plus-folders = +폴더 { $count }개
status-items = 항목 { $count }개
# $filter: 거르기 글(끝을 줄였을 수 있다), $shown / $of: 보이는 항목 / 모든 항목.
status-filter = 거르기: { $filter } · { $shown }/{ $of }
# $free: 크기(내림).
status-free = 남은 공간 { $free }
