# StackBite 조율자에 SwitchBite 연결

조율자가 프로젝트를 작업과 의존성으로 분해하고, Jev가 각 작업의 특성과 모델 풀의
설명을 판단해 담당 후보를 추천합니다. 조율자는 정확한 후보를 예약하고 StackBite의
허가된 위임 도구로 실행합니다. StackBite 원본 코드·설치·세션은 변경하지 않습니다.

```text
사용자 요청 → 조율자의 작업 분해·DAG 검증
                    ↓ 준비된 작업 + 실제 모델 풀
                 SwitchBite → Jev 1.14
                    ↓ 추천 또는 보류
             조율자의 권한·가용성 재확인 및 예약
                    ↓ StackBite의 위임 도구
                 모델 작업자 실행
                    ↓ 검증된 완료 근거
             조율자의 결과 기록·다음 작업 라우팅
```

## 한 번 연결

프로젝트에서 다음 명령을 실행합니다. `--judge jev`는 기본값이며 명시할 수도 있습니다.

```powershell
switchbite stackbite --judge jev --workspace C:\path\to\project --client both
```

Codex 프로젝트 설정 `.codex/config.toml`과 Claude 프로젝트 설정 `.mcp.json`에
`switchbite-stackbite` 서버를 등록합니다. `--client codex` 또는 `--client claude`로
하나만 고를 수 있습니다. 실행 파일은 사용자 데이터 폴더에 해시별로 보관합니다.
`--dry-run --json`은 변경 없이 계획을 출력합니다. 반복 설치는 멱등이며 변경 전에는
정확한 백업을 남깁니다. 다른 서버와 주석, 기존 `switchbite` 서버는 보존합니다.

Jev는 **새 조율자 프로세스 환경의 `TYPESAFE_API_KEY`**를 사용합니다. 설치기는 키의
실제 값을 읽거나 파일에 복사하지 않습니다. Codex에는 변수 이름 전달 허용을,
Claude에는 `${TYPESAFE_API_KEY:-}` 참조를 등록합니다. 키가 없거나 판단 실패 시
보류하며, 다른 판단 모델로 자동 대체하지 않습니다. 작업 요약과 후보 설명은 Jev의
외부 서비스에 전송되므로 비밀 값이나 불필요한 소스 원문을 넣지 마세요.

StackBite에서 해당 프로젝트의 **새 조율 세션**을 시작하고 프로젝트 신뢰/MCP 승인이
나오면 승인합니다. 기존 세션이 설정을 자동 갱신한다고 가정하지 않습니다.
`switchbite-stackbite`의 `switchbite_start`, `switchbite_next`, `switchbite_observe`로 작업을 시작하고 진행합니다. MCP 초기화 지침에 조율 순서가 포함됩니다.

기존 사용자 `switchbite`와 이름을 분리하여 동명 설정 충돌을 피합니다. 기업 정책,
CLI 옵션, 프로젝트 신뢰에 의해 프로젝트 MCP가 차단될 가능성은 남아 있습니다.
Claude는 `claude mcp get switchbite-stackbite`, Codex는
`codex mcp get switchbite-stackbite --json`으로 발견된 실행 경로를 확인할 수 있습니다.
CLI의 발견 확인은 모델이 실제 도구를 사용했다는 증거와 다릅니다.

## 조율자에게 내리는 요청 예

> 이 프로젝트를 의존성과 출력 소유권이 명확한 작업으로 분해하고 실제 작업자 풀과 함께 워크플로를 시작해 주세요. 요청된 작업을 실행하고 결과를 관찰하며 검증과 통합까지 진행하세요. 실패 시 필요한 맥락을 수집하고 제한된 범위에서 복구하세요.

`examples/stackbite-workflow.json`은 분해 계획, 역할 선호와 후보 설명, 라우팅 입력의
작은 합성 예입니다. 후보는 설치된 계정이나 실제 모델을 자동 발견한 결과가 아닙니다.
모델/effort/worker 인스턴스마다 별도의 ID와 사실에 근거한 설명을 제공해야 합니다.
실제 전체 모델 목록과 가용성은 조율자가 확인합니다. Jev는 CLI를 직접 실행하지 않습니다.

독립 작업은 출력 소유권과 예약이 분리된 경우 병렬로 진행합니다. 완료 근거를 검증하고 통합한 뒤 후속 작업을 시작하며, 결과가 불명확한 실행은 중복 실행하지 않고 확인합니다.

## 로컬 Laya도 같은 인터페이스 사용

```powershell
switchbite stackbite --judge laya --workspace C:\path\to\project --client both --endpoint http://127.0.0.1:11435 --model laya:multilingual
```

명시적으로 선택해야 하며 Jev의 키 전달 항목은 해당 서버에서 제거합니다. 준비와
운영 제약은 `docs/stackbite-laya.ko.md`를 참고하세요. Jev↔Laya 변경 후 새 세션을
시작해야 합니다. 이전 Laya 미리보기의 프로젝트 `switchbite` 항목은 자동 삭제하지
않으므로, 새 서버가 동작하는 것을 확인한 뒤 필요하면 그 항목만 제거하세요.

## 실행 경계

이 패키지는 프로젝트 범위의 coordinator MCP 플러그인입니다. 원본 StackBite 설치와 세션은 변경하지 않습니다. 워크플로 상태는 프로젝트에 저장되며, 호스트가 지원하는 실행과 검증 결과를 관찰해 진행합니다.
