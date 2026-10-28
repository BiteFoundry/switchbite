# CLI 실행·계획 복구 검증

코드 커밋: `358d51abe82f24e1190c9a1704d708ce22512728`.
버전: `0.9.0-preview.3`.
Windows 실행 파일 SHA-256: `342fe8f583cee2110417e42c658a0c93ef0fed9aeb13e70e74cff64f76041f2c`.

## 자동 검사

- Rust 검사 276개 통과, 환경이 필요한 검사 2개 ignored.
- 웹 모델·공개 미리보기 검사 8개 통과.
- `cargo clippy --all-targets -- -D warnings`, 포맷 검사, `git diff --check` 통과.
- 저장된 실제 실패 계획 2건을 재생해 등록, 모델 고정값과 작업 지침 보존, 적격 후보 존재를 확인했다.
- 잘못된 의존성을 반환하는 가짜 CLI가 구체적인 진단과 이전 계획을 전달받아 수정하는 경로를 확인했다.
- 대량 stdout/stderr 원문을 바이트 단위로 확인했다. 합계 1MiB를 넘는 로그와 단일 16MiB 초과 명령 출력 레코드가 완료 판정을 통과한다.
- 명시적 timeout과 사용자 취소는 프로세스 트리를 정리하고 기존 출력을 보존한다. 기본 deadline은 없다.
- 기존 웹 버전의 900초 자동 제한 마이그레이션, 실제 목표의 작업자 입력 전달, 오류의 단계·노드·필드 표시를 검사했다.

## 실제 Codex 작업

검증 실행: `run-1793165183918651900-80844`.
조율자·구현·독립 검토에 Codex CLI의 `gpt-6.1-terra`, `high`를 지정했다. argv의 요청값과 native 완료 이벤트를 확인했다. CLI가 별도 반환하지 않는 모델 검증값은 화면에서 미보고로 유지한다.

사용자 목표: HTML 파일의 title을 `SwitchBite CLI smoke`, 본문 h1을 `Routing works`로 작성하고 별도 세션에서 검토한다.

| 단계 | 결과 |
|---|---|
| 계획 | Codex 구조화 출력으로 2개 노드 반환·등록 |
| 모델 배정 | 명시한 실제 카탈로그 프로필 선택 |
| 구현 | `site/index.html` 생성·검증·통합 |
| 독립 검토 | 원래 사용자 목표와 HTML을 대조, title과 h1 모두 PASS |
| 최종 상태 | 2/2 완료, attention 없음 |

결과 브랜치: `switchbite/f45d50912c38f801f99f4177/integration`.
결과 커밋: `4b3857ce3b890d71270f0a7829637e0177996474`.
로컬 결과 경로: `C:\Users\user\AppData\Local\Switchbite\workspaces\d25c4d5705a4302b6d02ba1a\f45d50912c38f801f99f4177\integration`.
상세 실행 기록: `.switchbite/rca-smoke-runtime/verified-result.json`.

첫 실제 실행에서 사용자 MCP 시작 진단 때문에 정상 완료를 거절하던 경로를 발견해 수정했다. 그 실행의 검토 보고서에서 원래 목표가 누락된 것도 확인해 작업자 입력 전달을 수정한 뒤 위 실행으로 재검증했다. 첫 기록은 `.switchbite/rca-smoke-runtime/initial-review-inconclusive.json`에 보존했다.

## 로컬 적용

- 주소: http://127.0.0.1:58431/
- 앱 버전: `0.9.0-preview.3`.
- 엔진: SwitchBite 소유 캐시의 StackBite provider-stream `0.5.75`.
- 중단된 기존 실행 `run-1793162389339754200-52752`를 복원하고 사용자 재개 대기로 유지했다. 재개 시 수정된 계약과 이전 웹 자동 deadline 마이그레이션이 적용된다.
- 현재 로컬 채널과 런처는 `.switchbite/runtime-cli-recovery-release/`를 가리킨다. 이전 실행 파일과 채널은 보존했다.
- 기존 Rust target 디렉터리를 재사용했다. WorkBite Task binding이 없어 StorageGuardian에 새로 등록하지 않았다.

제품 변경은 SwitchBite 저장소와 자체 런타임 경로에 적용했다. StackBite 원본 코드·설치에는 쓰기 작업을 수행하지 않았다.
