# 실행 중 피드백 검증 — 2026-10-28

대상 버전: `0.9.0-preview.4`. SwitchBite의 보고 수집, 조율자 재계획, 실행 중단, 웹 입력 경로를 검증했다. 원본 StackBite는 수정하지 않았다.

## 실제 CLI 실행

실행 ID: `run-1793168388553783600-81520`. 조율자·작업자·독립 검토자는 실제 Codex CLI의 `gpt-6.1-terra / high`를 사용했다. 라우터는 offline 설정이었고 Jev 유료 API는 호출하지 않았다.

1. HTML의 title을 `Feedback demo`, h1을 `Alpha`로 만들고 독립 검토하는 2개 작업으로 시작했다.
2. 실행 중 웹의 **조율자에게 전달**에서 최종 h1을 `Beta`로 바꿔 달라고 요청했다. 기존 실행·완료 결과는 보존하도록 지정했다.
3. 조율자는 현재 상태를 읽고 완료된 원래 작업과 검토를 유지했다. `update_beta`와 그 뒤에 실행할 `review_beta`를 추가했다. 계획 변경은 1회였다.
4. 변경된 그래프의 **4/4 작업이 완료**됐다. 최종 HTML은 `<title>Feedback demo</title>`, `<h1>Beta</h1>`이며, 새 `review_beta.md`에서 두 조건 모두 PASS로 확인했다. 원래 `review.md`의 Alpha 검토도 실행 이력으로 보존했다.

결과 브랜치: `switchbite/9d02c0b7d224cb4db52a924e/integration`.
결과 커밋: `521418dc91a9eca4dcb7395ce4dd2edb382c60ee`.
로컬 원시 검증 기록: `.switchbite/feedback-validation/live-result.json`.

## 자동 검증

- `cargo clippy --all-targets -- -D warnings`: 통과.
- `cargo test --tests --quiet`: **303 통과, 0 실패, 2 ignored**. 실행 로그: `.switchbite/feedback-validation/cargo-tests.log`.
- `node --test tests/workflow_ui_model.mjs tests/pages_preview.mjs`: **9 통과**.
- `node --check assets/workflow-ui/app.js`, `git diff --check`: 통과.

핵심 사례는 다음과 같다.

- 보고 ID의 재전송은 중복 처리하지 않고, 조율 중 들어온 새 보고는 다음 판단까지 보존한다.
- 대상 작업의 후속 배정은 기다리되 관련 없는 병렬 작업과 기존 결과의 검증·통합은 계속한다.
- 완료·실행·이미 청구한 부작용 작업을 재계획에서 임의로 제거하지 않는다.
- 작업자가 `blocked`를 보고하면 native CLI가 정상 종료해도 성공 산출물로 통합하지 않는다. 실행별 보고 원문은 보존하고 제어 파일은 결과에서 제외한다.
- fake native 실행으로 `launching` 단계 프로세스의 실제 종료를 확인했다. 중단 확인 뒤 시스템 보고와 새 재계획이 이어진다. 이 중단 사례는 실제 모델 작업을 강제 종료하는 E2E와 구분한다.
- 중단 판단보다 결과 완료가 먼저 도착하는 경우 결과를 보존하고 최신 상태를 다시 조율한다.
- MCP 서버를 재시작해도 보고·결정이 유지되고 동일 요청의 중복 적용이 없다.

사용법과 외부 설계 비교 근거는 [실행 중 보고와 재계획](live-feedback.ko.md)에 정리했다.
