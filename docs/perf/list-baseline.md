# 폴더 목록 성능 기준 측정

측정 명령: `cargo bench -p rf-vfs --bench list`

판정은 PRD 7.1 기준 환경의 값으로만 하며 그 밖의 행은 참고용이다.

공급자 몫은 PRD 목표의 50%이고, 나머지 50%는 뷰모델과 렌더링 몫이다.

- `list/1k_full` ≤ 25 ms (PERF-03 50 ms)
- `list/100k_first_batch` ≤ 250 ms (PERF-04 첫 화면 500 ms)
- `list/100k_full` + `sort/100k_name` ≤ 1,000 ms (PERF-04 전체 로드 2초)

각 칸은 criterion이 `time:` 줄에 찍은 가운데 값(중앙값 추정)이다.

| 날짜 | 커밋 | 환경(CPU, RAM, 디스크, OS) | list/1k_full | list/100k_first_batch | list/100k_full | sort/100k_name | raw/100k_read_dir | 판정 |
|---|---|---|---|---|---|---|---|---|
| 2026-10-23 | `7fcdc2a` | Intel Xeon @ 2.80GHz 4코어, RAM 16 GB(MemTotal 16,477,348 kB), 디스크 ext4(`/dev/vda`, `/tmp`가 루트와 같은 볼륨), Ubuntu 24.04.5 LTS 커널 6.18.44-fc-v153. 공유 클라우드 컨테이너라 이웃 작업의 영향을 받을 수 있다 | 1.4006 ms | 3.5555 ms | 229.30 ms | 627.26 ms | 34.597 ms | 참고 |

## 측정 메모

- 커밋 칸은 측정한 공급자 코드(`LocalProvider`와 `sort_entries`)가 들어 있는 커밋이다.
- 픽스처는 시스템 임시 폴더의 `RacoonFinder-bench-1k`(파일 950 + 폴더 50)와 `RacoonFinder-bench-100k`(파일 95,000 + 폴더 5,000)다. 파일은 모두 0바이트이고, 10개 중 하나는 `사진 NNNNNN.jpg`, 나머지는 `file_NNNNNN.txt`다. 처음 돌릴 때 만들고 옆의 `.complete` 표시 파일로 재사용한다. 폴더는 수동으로 지워도 된다.
- 픽스처는 두 번째 실행부터 OS 캐시에 올라 있다. 첫 실행의 콜드 캐시 값은 기록하지 않는다.
- `sort/100k_name`은 복제한 벡터를 정렬하고 그 벡터를 버리는 시간까지 잰다.
- `raw/100k_read_dir`은 같은 폴더를 `std::fs::read_dir`로 읽어 이름만 센 값이다. `list/100k_full`과의 차이가 공급자가 더한 몫(항목마다 `stat`과 `Entry` 만들기)이다.
