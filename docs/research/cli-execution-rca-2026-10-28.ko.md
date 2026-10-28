# SwitchBite CLI 실행 방해 요인 RCA

조사일: 2026-10-28. 소스 기준: `56e180091e2f916b936e7a35a83cfba416301c45`.
실행 중 앱: `0.9.0-preview.2`, 빌드 커밋 `d1f21eeac7cec332126aec4a6b022841f6ae5692`.
실행 파일 SHA-256: `4118ce70f70f70b04dcb631ee784a8e21aeb45dceca1e206bbbdf3cd9ecc3839`.
해당 실행의 어댑터: StackBite provider-stream `0.5.75`, 소스 `5866c4fa78e925cbac4d1c1dbedfa241099db7fb`. Codex CLI: `0.164.0`.

## 결론

최근 조율자의 계획 반환 실패는 SwitchBite의 계획 계약 불일치가 직접 원인이다. CLI는 두 번 모두 정상 종료했고 계획 파일도 작성했다. 잘못된 예시를 준 프롬프트와 다른 형식을 요구하는 검증기가 충돌한다.

계획의 라우팅 형식을 고치면 다음 단계에서 자연어 지침을 필수 기능 ID로 해석하는 문제가 발생한다. 이 문제까지 고쳐야 모델 배정이 진행된다. 시간 제한만 늘리면 두 문제 모두 남는다.

별도로, 기존 작업에는 15분 강제 종료와 출력량 초과 강제 종료가 실제로 발생했다. 평소 CLI와 실행 조건이 달라지는 부분도 확인했다.

## 1. 최근 실패 재현

대상: `run-1793162389339754200-52752`, 계획 action 1.

| 시도 토큰 | 실행 시간 | 프로세스/CLI | 계획 파일 | SwitchBite 판정 |
|---|---:|---|---|---|
| `1a7cfaeadd46fe684604b5c6` | 130.031초 | exit 0, `turn.completed`, 완료 영수증 검증 성공 | 32,399바이트, 8개 노드 | `invalid_receipt` |
| `ed51b6d48098dbe70ec1f1b8` | 165.155초 | exit 0, `turn.completed`, 완료 영수증 검증 성공 | 19,043바이트, 8개 노드 | `invalid_receipt` |

두 실행의 `.backend.json`, `.jsonl`, `executions/*.json`, `actions/*.json`, 워크트리의 `switchbite-observation.json`, 웹 세션 오류를 대조했다. 계획 크기는 128KiB 한도 이내였고, 실행 시간도 900초 한도 이내였다.

### 직접 원인: 프롬프트와 검증기의 라우팅 위치 불일치

[조율자 프롬프트](../../src/web_coordinator.rs)의 `NODE_FORMAT` L12는 다음 형태를 예시로 준다.

```json
{"id":"audit","provider":"codex","model":"gpt-6.1-terra","effort":"high"}
```

그러나 [계획 검증기](../../src/workflow/mod.rs) L1539–1576은 최상위 `provider`, `model`, `effort`를 허용하지 않는다. 올바른 형태는 다음과 같다.

```json
{"id":"audit","routing":{"provider":"codex","model":"gpt-6.1-terra","effort":"high"}}
```

실패한 두 계획의 모든 노드에 최상위 모델 필드가 있었다. 동일한 실행 바이너리에 저장된 결과를 다시 넣었다. 원본은 두 건 모두 `invalid_receipt`; 세 필드의 위치만 `routing` 안으로 옮기면 두 건 모두 8개 작업이 등록됐다. 내용, 의존 관계, 모델 선택은 바꾸지 않았다.

재현 스크립트: `.switchbite/rca-plan-replay.mjs`.
결과: `.switchbite/rca-plan-replay-20261028/results.json`.
실제 사용자 실행을 재개하거나 변경하지 않고 별도 진단 디렉터리의 workflow start/observe만 사용했다. 추가 AI 호출은 0건이다.

### 다음 차단점: 자연어 requirements를 기능 ID로 처리

두 계획의 `requirements`에는 작업 수행 지침이 들어 있다. [카탈로그 shortlist](../../src/catalog/mod.rs) L716–739, [라우팅](../../src/routing.rs) L196–201, [명시 배정](../../src/route_context.rs) L114–122는 각 값을 후보의 `capabilities`와 정확히 일치시킨다.

실제 후보의 기능은 `file.read`, `file.write`, `code.explore` 등이다. 문장으로 적힌 지침은 이 ID와 일치하지 않는다. 저장된 실제 카탈로그와 대조한 결과, 라우팅 위치를 수정한 두 계획의 **16개 노드 모두 적격 후보 0명**이었다. 요청한 Terra/Sol 프로필은 존재한다. 문제는 모델의 부재가 아니라 조건의 의미다.

필수 기능 ID와 작업 지침을 분리해야 한다. 현재 API를 유지하는 최소 수정은 `requirements`에는 지원하는 기능 ID만 넣고, 자연어 지침은 `objective`에 보존하는 것이다. 자연어 지침을 삭제하거나 모델 제약을 풀어 해결하지 않는다.

### 반복 원인: 검증 결과가 재시도 입력에서 빠짐

[observe](../../src/workflow/mod.rs) L523–524는 구체적인 계획 검증 실패를 `invalid_receipt` 하나로 바꾼다. [웹 실행 루프](../../src/web_workbench.rs) L763–779는 이를 표시하고 종료한다. 재개 시 attempt는 증가하지만, [fulfill](../../src/web_coordinator.rs) L159–170은 사용자 목표·action·workflow 상태로 다시 입력을 구성한다.

저장된 두 `.prompt`의 `instructions`는 동일했다. 두 입력 모두 직전의 `invalid_receipt`와 잘못된 필드 위치에 관한 진단을 포함하지 않았다. 재시도는 같은 오답 예시를 다시 제공한다. 이미 만든 계획을 국소 수정할 수 있는데도 CLI 전체 계획 작업을 반복한다.

## 2. 시간과 출력 제한의 실제 영향

두 workspace의 저장된 coordinator/host 실행 레코드를 조사했다. status가 있는 47건 중 `completed` 39건, `timeout` 4건, `output_limit` 2건, `failed` 1건, `cancelled` 1건이었다. status 필드가 없는 이전 형식 10개는 이 집계에서 제외했다. 프로세스 완료는 workflow 계획 승인·결과 검수 완료와 별개다.

| 제한 | 실제 동작 | 확인 결과 | 수정 방향 |
|---|---|---|---|
| Web 작업 900초 | 조율자·작업자가 같은 host 설정 사용. 출력을 내고 있어도 경과 시간이 차면 프로세스 트리 종료 | 과거 작업 4건이 900.15–900.17초에 종료 | 전체 작업 시간과 무응답 관찰을 분리. 정상 진행 중인 작업을 기본 15분에 강제 종료하지 않기. 사용자 지정 deadline은 유지 |
| stdout/stderr 각각 1MiB | 초과 즉시 프로세스 트리를 종료 | 과거 2건의 `output_limit` 확인 | 실행과 화면 표시 한도 분리. 디스크 로그에 기록하고 UI만 tail/page 방식으로 표시 |
| 초과 로그 tail 64KiB | 원본 로그 파일 자체를 끝부분으로 덮어씀 | 초과 종료 레코드의 보존 로그가 약 64KiB | 원본을 보존하고 표시용 tail만 따로 읽기 |
| 조율자 입력 120KiB, host prompt 128KiB | 실행 전에 거절. Codex는 stdin 파일로 전달 | 최근 입력은 6,469바이트로 미해당 | 오류 위치와 크기를 표시. 계획·재계획에는 필요한 상태만 전달 |
| 조율자 결과 128KiB | 정해진 파일만 읽고, 파일 부재·크기·JSON 오류를 거절 | 최근 두 결과에는 미해당 | 구조화된 최종 결과를 우선 회수하고 단일 스키마로 검증 |
| Jev 판단 30초, queue 5초 | 모델 배정 판단 제한 | Codex 계획 실행 시간 제한과 다른 경로 | 판단 API와 CLI 작업 오류를 분리해서 표시 |
| activity 30초/120초 | `quiet`/`quiet_long` 상태 표시 | 이 경과 자체로 CLI를 종료하지 않음 | 상태 안내로 유지 |
| 엔진 capabilities 3초 | 작은 사전 probe에만 적용 | 실제 CLI 실행에는 이 3초가 적용되지 않음 | probe 실패와 실행 실패 구분 |

근거: [host timeout/종료](../../src/adapters/mod.rs) L439–443, L691–747; [로그 덮어쓰기](../../src/adapters/mod.rs) L327–348; [웹의 900초 고정값](../../src/web_workbench.rs) L399; [판단 제한](../../src/runtime.rs) L20–21; [상태 관찰](../../src/activity.rs) L391–405; [engine probe](../../src/adapters/stackbite_backend.rs) L13–21.

최근 이전 run `run-1793116621484147900-56208`에는 timeout 3건과 output_limit 1건, 과거 Jev-Router run `run-1793100449736752900-37172`에는 timeout 1건과 output_limit 1건이 있었다. output_limit에 걸린 JSONL에는 명령 출력 또는 MCP의 큰 응답이 포함되어 있었다. 화면에서 많은 출력을 만드는 정상 조사도 현재 종료 조건에 걸릴 수 있다.

## 3. 평소 CLI와 달라지는 실행 조건

아래 항목은 코드를 확인한 제약이다. 최근 계획 실패의 직접 원인으로 확인된 항목과 구분한다.

| 구간 | SwitchBite가 추가하는 조건 | 실행에 미치는 영향 |
|---|---|---|
| Codex 설정 | `--ignore-user-config` | 사용자 `config.toml`의 MCP·provider·기능 설정을 읽지 않음. CLI help는 인증에 CODEX_HOME을 계속 쓴다고 명시. 로그인 여부와 도구 설정 승계는 다른 문제 |
| Codex 권한 | `workspace-write`, Windows `unelevated`, approval `never` 고정 | 작업 폴더 밖 쓰기 등 일반 CLI에서 허용받아 수행하던 작업이 거절될 수 있음. Web에서 필요한 권한 요청에 답하는 경로도 없음 |
| Codex 세션 | `--ephemeral`, 매 작업 새 exec | 기존 대화·도구 상태를 이어받지 않음. 웹의 재개는 기존 Codex thread의 resume과 다름 |
| Claude 도구 | `--restricted`, `--safe-mode`, `Read,Write,Edit`만 허용, MCP 빈 설정, hooks 비활성 | 파일 편집은 가능하지만 Bash·빌드·테스트·일반 MCP 작업을 실행할 수 없음. 카탈로그의 `code.change` 등과 실제 도구 권한이 일치하는지 확인 필요 |
| 기타 CLI 설정 | Antigravity sandbox/accept-edits, Cursor trust/force, OpenCode standalone/auto | CLI마다 권한 정책이 다름. 동일한 작업자 능력으로 취급하면 실제 실행 범위를 잘못 배정할 수 있음 |
| 환경변수 | StackBite/WorkBite/SwitchBite 계열, 일부 CODEX 식별자, TYPESAFE_API_KEY 등 제거 | 부모 seat를 잘못 제어하지 않도록 격리하지만 해당 도구·키가 필요한 업무에는 별도 전달 계약 필요. 일반 로그인용 환경 전체를 제거하는 코드는 아님 |
| Windows 명령 길이 | Codex만 prompt 파일을 stdin으로 전달. 나머지는 argv 전달 | 긴 계획/작업 설명은 Windows 명령행 한도에 먼저 걸릴 수 있음. host의 128KiB 입력 허용과 OS argv 한도는 일치하지 않음 |
| 작업 경로 | 별도 Git worktree를 지정 HEAD에서 생성 | 사용자가 작업 폴더에서 보던 미커밋 파일·ignored 설정/설치물은 자동 복제되지 않음. 일반 CLI에서 되던 실행이 격리 작업 폴더에서 달라질 수 있음 |
| 완료 영수증 | JSONL 전체가 파싱 가능해야 하며 native 완료 이벤트 순서를 요구 | exit 0만으로 성공 처리하지 않음. 이벤트 형식 변경·오염된 stdout·정상 복구된 오류 이벤트에 대한 호환성 시험 필요. 최근 두 실행은 이 검사 통과 |
| 자식 프로세스 | 부모 종료뿐 아니라 Windows job의 모든 프로세스 종료를 기다림 | CLI가 서버/watch를 남기면 turn 종료 후에도 host가 기다려 15분 종료까지 갈 수 있음. 최근 두 실행에는 이 대기 발생 증거 없음 |
| 앱 소유권 | kill-on-close Windows job | 앱 프로세스가 종료되면 소유한 작업도 종료. 브라우저 탭을 닫는 동작 자체와는 구분해야 함 |
| 병렬성 | 요청한 max_parallel 외 provider resource_capacity 기본 3, 경로 중첩 직렬화 | 동시 실행을 6으로 골라도 동일 provider 작업은 3개로 제한될 수 있음. 경로 소유권이 겹치면 추가 직렬화. 최근 실행은 계획 승인 전에 막혀 작업자 0명 |
| 산출물 통합 | 선언한 write_paths 밖 변경을 거절; integration dirty 상태도 거절 | lockfile·테스트·생성 파일이 계획에 빠지면 CLI 구현이 끝나도 통합 실패 가능 |
| 복구 | repair_rounds 기본 2. task retry는 cancel 후 복구, CLI stdin 입력 전송 API 없음 | 권한/질문 대기 작업에 웹에서 답하거나 같은 native 세션을 이어가는 기능은 현재 없음. 기존 결과와 로그를 보존해 재계획에 반영해야 함 |
| 후보 발견 | 후보에 `readiness: unverified`; 공통 기능 목록 부여 | 설치·모델 목록 발견이 실제 권한·도구·유료 한도 사용 가능성까지 입증하지는 않음. 사실과 다른 기능을 라우터에 제공하면 잘못된 배정 발생 |

근거: [CLI argv/영수증](../../src/adapters/workers/mod.rs) L5–126, L146–280; [입력·환경·결과](../../src/adapters/mod.rs) L763–879; [작업 폴더 생성](../../src/adapters/mod.rs) L539–585; [프로세스 소유권](../../src/adapters/process.rs) L58–82, L117–130; [provider 용량](../../src/catalog/mod.rs) L233; [쓰기 경합과 용량](../../src/workflow/mod.rs) L1122–1178; [통합 경로 검사](../../src/adapters/mod.rs) L1010–1081; [재시도](../../src/web_workbench.rs) L603–661.

## 4. 정상 동작으로 확인된 경로

- 최근 Codex 입력은 파일 stdin으로 전달되었고 EOF도 전달됐다. 실행이 끝난 두 건에서 stdin 무한 대기는 원인이 아니다.
- StackBite 어댑터의 실제 `exec` 경로는 argv와 원시 stdout/stderr를 전달하고 stdin을 닫는다. 해당 경로 자체에 추가 3초/30초 실행 제한은 없다. 레거시 one-shot 모드의 별도 상수를 현재 bridge 실행 제한으로 계산하지 않았다.
- UI의 tail 64KiB/최대 128개 entry 표시와 host의 1MiB 강제 종료는 별개다. 화면 표시 한도를 늘리는 것만으로 프로세스 종료는 해결되지 않는다.
- 최근 로그의 중첩 PowerShell 명령에는 quoting 오류가 있었으나 CLI가 다른 명령으로 검증을 마치고 정상 종료했다. 이번 `invalid_receipt`의 직접 원인이 아니다.
- 화면의 조율자 Sol high는 저장된 실제 실행 argv와 일치했다. Terra 작업자들은 계획 승인 전이라 시작되지 않았다.

## 5. 수정 순서와 완료 기준

1. **계획 계약 통일.** workflow의 스키마를 조율자 입력·검증에 함께 사용한다. `routing` 예시 수정, 기존 flat 필드는 충돌 없는 경우만 같은 의미로 정규화한다. `requirements`는 기능 ID, 작업 지침은 objective로 명확히 구분한다. 완료 기준: 저장된 실패 계획 2개가 사용자 모델 지정과 지침을 보존하며 등록되고 실제 적격 후보를 가진다.
2. **계획 회수와 재시도 연결.** Codex의 `--output-schema`와 구조화된 최종 결과 회수를 사용한다. 파일 출력은 기존 호환 경로로 유지할 수 있다. 검증 오류에 노드 ID·필드·기대 형식을 넣고 기존 계획을 수정하도록 전달한다. 완료 기준: 잘못된 필드 하나가 전체 계획 재생성 루프로 이어지지 않는다.
3. **작업 중단 조건 수정.** 출력량 때문에 CLI를 종료하지 않도록 로그 저장과 표시를 분리한다. 15분 hard limit을 정상 진행 여부와 무관한 기본 중단 조건으로 쓰지 않는다. 완료 기준: 1MiB를 넘기는 출력과 15분 넘는 정상 진행을 보존하고, 명시적 취소/deadline은 동작한다.
4. **CLI 실행 프로필 정리.** 선택한 CLI의 실제 도구·권한·사용자 설정 승계 범위를 라우터와 일치시킨다. Claude에 빌드 업무를 배정하면서 Bash를 제거하는 모순을 없앤다. 지원하는 CLI는 stdin/file 입력을 사용한다. 완료 기준: 같은 로그인·모델·권한·작업 스냅샷에서 직접 CLI와 wrapper 실행의 차이가 설명되고 필요한 도구가 실제 사용 가능하다.
5. **복구와 진행 표시 정리.** CLI 완료/계획 검증/라우팅/실행/통합을 서로 다른 상태와 오류로 표시한다. 재시도가 새 세션인지 기존 세션 재개인지 드러내고, 필요한 입력은 그 내용을 보존한다. 완료 기준: 사용자에게 로그인·작업 재입력을 요구하기 전에 실패한 단계와 기존 산출물을 확인할 수 있다.

프로토콜 선택 근거: [OpenAI의 비대화식 CLI 문서](https://learn.chatgpt.com/docs/non-interactive-mode)는 JSONL 이벤트와 JSON Schema 최종 응답을 설명한다. 로컬 Codex 0.164.0 `exec --help`에서도 stdin `-`, `--output-schema`, `--output-last-message`, `--ephemeral`, `--ignore-user-config` 동작을 확인했다. 이 RCA는 새 agent 프레임워크나 별도 런타임 도입을 요구하지 않는다.

조사 중 제품 소스, 실행 중 workflow, StackBite 원본을 수정하거나 새 AI 업무를 실행하지 않았다. 수정 사항의 실제 provider 통합 검증은 후속 구현에서 수행한다.
