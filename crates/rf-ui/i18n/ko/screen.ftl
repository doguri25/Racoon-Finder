# 이름이나 키가 들어가는 창의 말: Rust가 만들어(words.rs) 창의 속성이나 모델의 칸으로 준다.
# 인자가 없는 창의 말은 ui.ftl(`Tr`)에 있다.

# 경로 조각 뒤의 ›: 그 폴더의 하위 폴더 목록을 연다. $name: 그 조각의 이름.
crumb-subfolders = ‘{ $name }’ 하위 폴더
# 줄 앞 체크박스의 이름(화면 읽기 프로그램). $name: 줄의 이름.
row-select = { $name } 선택
# 빠른 보기 아래의 안내. $key: 빠른 보기를 닫는 키(Space).
quick-look-hint = ← → 이전·다음 파일 · { $key } 닫기
# 작업 패널의 최근 완료 머리. $count: 끝난 작업의 수.
job-panel-recent-head = 최근 완료 { $count }개
