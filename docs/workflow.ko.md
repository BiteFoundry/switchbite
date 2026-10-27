# SwitchBite workflow

조율자가 목표를 작업 그래프로 분해하면 SwitchBite가 Jev/Laya 배정, 작업 공간 준비,
실행, 검사, 통합과 후속 작업 배정을 이어갑니다. Rust 엔진과 Windows 네이티브
실행기를 사용합니다. StackBite 원본 설치·코드·기존 세션은 유지합니다.

## 프로젝트 연결

```powershell
switchbite stackbite --judge jev --client both
```

프로젝트에 `switchbite-stackbite` MCP 서버를 등록합니다. 새 Codex/Claude 조율
세션에서 프로젝트 MCP 승인을 마치고 목표를 요청합니다. Jev는 조율자 환경의
`TYPESAFE_API_KEY`를 사용합니다. 로컬 Laya는 `--judge laya --endpoint URL --model NAME`
으로 선택하며 기존의 로컬 모델 서버를 사용합니다.

새 프로필의 도구는 세 개입니다.

- `switchbite_start`: 목표 또는 계획과 실제 사용 가능한 worker/model pool로 실행 생성.
- `switchbite_next`: 실행 시작·진행 확인·상태/일시정지/재개/취소. 로컬 작업자 정보가
  있으면 한 번의 next로 백그라운드 실행기가 알려진 DAG를 계속 처리합니다.
- `switchbite_observe`: 조율자가 작성한 계획·수정 계획·갱신한 모델 풀 등 결과 전달.

조율자는 필요할 때 status를 확인하고 plan/repair/refresh_pool 액션에 응답합니다.
알려진 독립 작업은 실행기가 병렬로 처리합니다. MCP 프로세스가 종료되면 재연결한
세션의 next로 기록을 확인하고 이어갑니다. 원래 실행기의 종료와 작업자 종료를
확인한 시도는 실패로 복구해 보완할 수 있으며, 성공 영수증은 중복 실행하지 않습니다.

## 실행 계약

계획의 각 노드는 `id`, `objective`, `depends_on`, `acceptance`, `outputs`와 선택적
`role`, `requirements`, `context_refs`, `write_paths`를 가집니다. 조율자가 이 정보를
작성합니다. `outputs`는 결과물 참조, `write_paths`는 저장소 쓰기 범위입니다.
`write_paths`는 파일·디렉터리·glob 또는 프로젝트 전체 `.`를 지원합니다.

작업자 프로필마다 `id`, `description`, `capabilities`, `available`을 제공합니다.
로컬 실행은 `provider`, `executable`, `model`, `effort`를 추가합니다. 현재 로컬
실행 provider는 Claude와 Codex이며, 판단기는 전체 후보 중 역량·가용성·정책을
만족하는 프로필을 비교합니다. 같은 CLI의 모델·설정·인스턴스 조합은 별도 ID를
사용합니다. `resource_key`로 같은 계정/작업자의 동시 사용을 조절할 수 있습니다.

각 시도는 전용 worktree를 사용합니다. 모델 완료, 작업 검사, 통합 후 검사가 모두
확인되면 의존 작업을 해제합니다. 실패한 시도의 작업 공간은 보존하고 보완 작업은
새 작업 공간에서 시작합니다. 검사는 `{program,args}` 배열로 지정하며 재개 시
다시 실행할 수 있는 프로젝트 검증 명령을 사용합니다.

코어는 예약, 실행 식별자, 커밋, 검사 요약, 이벤트를 프로젝트 `.switchbite/workflow`
아래에 기록합니다. 일시정지는 새 작업 배정을 멈추고 진행 중인 작업의 결과를
수집합니다. 취소는 소유한 Windows 프로세스 묶음을 정리합니다.

## CLI

프로젝트 폴더에서 실행합니다. `examples/workflow-minimal.json`의 경로와 모델 프로필을
실제 환경에 맞게 채웁니다.

```powershell
switchbite workflow run --request-file C:\project\workflow.json
switchbite workflow status RUN_ID
switchbite workflow next RUN_ID
switchbite workflow view RUN_ID --port 58420
switchbite workflow pause RUN_ID
switchbite workflow resume RUN_ID
switchbite workflow cancel RUN_ID
switchbite workflow continue RUN_ID
```

`run`, `next`, `continue`는 로컬 작업을 처리하고 완료 또는 조율자 입력이 필요한
시점에 결과를 출력합니다. `start --request-file FILE`은 실행만 생성합니다.
`observe RUN_ID --request-file FILE`로 조율 결과를 기록하고 next로 이어갑니다.
goal만 제출했다면 plan 액션에 작업 그래프를 답합니다. 코드 수정이나 모델 풀 갱신은
기존 실행의 해당 액션에 응답하므로 새 실행을 만들 필요가 없습니다.

## 화면

`workflow view`가 출력한 루프백 주소에서 의존성 그래프, 담당 모델, 작업 단계와
검사·통합 근거를 봅니다. 완료된 선행 작업은 실선, 대기 중인 의존성은 점선으로
연결합니다. 작업을 선택하면 상세 근거가 펼쳐집니다. 일시정지·재개·취소를 제공합니다.
기존 MCP 조율 프로세스 또는 CLI 실행 루프가 실행을 진행하며 화면은 같은 기록을 봅니다.

## 설계 출처

작업 그래프, 독립 worktree, 통합 후 리뷰 흐름은 Matt Pocock의 MIT 라이선스
[implement-spec](https://github.com/mattpocock/skills/blob/c612defa9e5cf9372262d0279eb124993ba77b71/skills/engineering/implement-spec/SKILL.md)
및 같은 저장소의 to-tickets 지침을 참고했습니다. 실행 상태 관리와 어댑터는 이
저장소에서 Rust로 구현했습니다.
