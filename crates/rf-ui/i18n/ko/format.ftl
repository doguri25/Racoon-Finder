# 크기, 걸린 시간, 종류 이름: 파일 목록과 다른 화면이 쓰는 꼴 (format.rs).

## 크기 (format::format_size). $bytes는 1024보다 작은 수, $size는 보이는 그대로의 수(1.4, 214).

size-bytes = { $bytes } 바이트
size-kb = { $size } KB
size-mb = { $size } MB
size-gb = { $size } GB
size-tb = { $size } TB

## 걸린 시간과 남은 시간 (words::took, words::time_left)

duration-under-a-second = 1초 미만
duration-seconds = { $seconds }초
duration-minutes = { $minutes }분
duration-minutes-seconds = { $minutes }분 { $seconds }초
duration-hours = { $hours }시간
duration-hours-minutes = { $hours }시간 { $minutes }분

## 종류 열 (format::kind_label). $ext는 대문자 확장자(PNG).

kind-app = 응용 프로그램
kind-folder = 파일 폴더
kind-link = 바로 가기
kind-special = 특수 파일
kind-file = 파일
kind-pdf = PDF 문서
kind-text = 텍스트 문서
kind-markdown = Markdown 문서
kind-word = Word 문서
kind-excel = Excel 통합 문서
kind-powerpoint = PowerPoint 프레젠테이션
kind-hwp = 한글 문서
kind-image = { $ext } 이미지
kind-audio = { $ext } 오디오
kind-video = { $ext } 동영상
kind-archive = { $ext } 압축 파일
kind-other = { $ext } 파일
