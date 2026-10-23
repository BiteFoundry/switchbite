# 오케스트레이터가 Switchbite를 사용하는 흐름

새 기본 MCP 프로필은 `agent`입니다. 도구는 네 개입니다.

| 도구 | 언제 쓰는가 |
| --- | --- |
| `switchbite_route` | 새로 배정할 준비가 된 작업의 담당자를 추천받을 때 |
| `switchbite_validate_plan` | 이미 작성한 DAG를 검증할 때 |
| `switchbite_feedback` | 호스트가 관측한 완료·실패·취소 결과를 정규화할 때 |
| `switchbite_status` | 버전과 판단 엔진 설정 여부를 확인할 때 |

## 처음 연결하기

```powershell
switchbite codex
# 또는: switchbite claude / cursor / grok / antigravity / opencode
```

압축을 푼 실행 파일이 PATH에 없다면 `.\bin\switchbite.exe codex`로 실행합니다.
선택한 CLI의 사용자 설정에 자동 등록합니다. 새 CLI 세션을 열고, 클라이언트가
요청하면 최초 한 번 서버 사용을 승인합니다. 이후 coordinator용 지침이 자동으로
전달되므로 설정 조각을 복사하거나 프로젝트 지침을 편집할 필요가 없습니다.

실행 파일은 `%LOCALAPPDATA%\Switchbite\bin` 아래에 별도 보관하므로 압축을 푼
폴더를 이동해도 연결을 유지합니다. 기존 설정과 주석을 보존하고 원본을 인접한
`.switchbite-*.bak` 파일로 백업합니다. 같은 명령을 반복해도 중복 등록하지 않습니다.
`--dry-run`은 쓰기 없이 연결 대상을 확인하고, `--json`은 짧은 결과를 반환합니다.
이름이 같은 다른 서버나 잘못된 설정은 덮어쓰지 않고 중단합니다.

Jev·Laya 등의 판단 모델 선택과 API 키 설정은 연결과 별도입니다. 연결 명령은
모델이나 키 값을 바꾸지 않습니다. 현재 배포판의 온라인 판단 구현은 Jev 1.14이며,
다른 판단 백엔드를 연결하는 기능이 추가된 것은 아닙니다. 클라이언트 정책과
프로젝트별 설정이 우선할 수 있습니다. OpenCode의 inline 환경변수 설정은 원본
설정에서 병합해야 하며, 파일은 `--config-file FILE`로 직접 지정할 수 있습니다.
세부 동작은 [연결 문서](connection.md)를 참고하세요.

기존 `setup --client NAME`과 `config --client NAME`은 계속 설정을 출력하기만 합니다.

## 한 번 호출하기

일반 입력에는 `task`와 `pool`이 필요합니다. 작업의 `id`, `objective`와 후보의
`id`, `description`, `capabilities`, `available`을 전달합니다. 관측하지 못한
가용성이나 역량을 임의로 채우지 않습니다. 나머지는 기본값으로 시작하면 됩니다.

```json
{
  "task": {"id": "csv:parse", "objective": "CSV 입력 검증과 회귀 테스트 구현"},
  "pool": [{
    "id": "worker:implementation",
    "description": "호스트가 허용한 Rust 구현 작업자",
    "capabilities": ["code.change"],
    "available": true
  }]
}
```

응답은 `status`, `next_action`, `summary`, `candidate_id`부터 읽습니다.
텍스트에도 요약과 다음 행동을 짧게 표시하고, 전체 점수와 스냅샷은 구조화된
데이터로 제공합니다. `recheck_and_dispatch`는 호스트가 현재 권한·용량을 다시
확인하고 예약할 수 있다는 제안입니다. 실행 완료나 실행 권한을 증명하지 않습니다.

보류라면 다음 행동에 따라 작업을 명확히 하거나, 후보 풀을 갱신하거나,
리뷰를 요청하거나, 진행 중인 작업의 완료를 기다립니다. 후보를 임의로 골라
보류를 우회하지 않습니다. 오류에는 정적 코드와 재시도 가능 여부를 제공합니다.

## 필요할 때만 정책 추가하기

`policy.profile`은 `balanced`, `quick`, `implementation`, `review`, `mockup`
중에서 선택합니다. 프로필은 가중치와 판단 기준을 바꾸며, 모델 역량이나 계정
권한을 만들어내지 않습니다. 고급 호스트는 가중치·최소 기준·동점 정책·예산·
지연 한도·리뷰 작성자 제외 조건을 명시할 수 있습니다.

단계가 바뀔 때 기존 담당자와 진행 상태를 continuity로 전달하면 맥락을 유지하는
정책을 적용할 수 있습니다. 진행 중인 작업을 새 후보에게 자동 이동하지 않습니다.
코딩 모델 자체의 컨텍스트를 다른 CLI로 복사하거나 복원하는 기능은 호스트가 맡습니다.

## 호환성과 평가

API 1 도킹과 `serve --profile cli`는 기존 입력을 유지합니다. 새 기본 설정은
`serve --profile agent`를 사용합니다. 고급 snapshot/replay는 네이티브 API에서
작업·후보·정책 일치를 검사하며, 일반 MCP 입력에는 노출하지 않습니다.

`switchbite evaluate --request-file FILE`로 호스트가 기록한 작업 결과를 비교할 수
있습니다. 합성 데이터와 실제 관측 데이터를 구분하며, 누락된 결과는 성공이나
비용 0으로 간주하지 않습니다. 평가가 실제 운영 정책을 자동으로 바꾸지는 않습니다.
