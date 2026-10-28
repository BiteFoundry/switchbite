# 실행 중 보고와 재계획

실행 중인 업무의 방향을 바꾸거나 문제가 생기면 **조율자에게 전달**에 내용을 입력한다. 대상 작업을 고르면 해당 작업과 후속 작업을 조율 대상으로 잡는다. 전체 작업을 고르면 남은 계획 전체를 검토한다. 보고·응답과 추가·변경·제거된 작업은 화면에 남는다.

조율자는 현재 작업 상태와 보고를 읽고 남은 작업, 의존 관계, 배정 조건을 조정한다. 완료된 결과와 실행 중인 작업은 유지한다. 기존 작업을 바꿔야 할 때는 종료를 확인한 뒤 새 시도를 배정한다. 완료된 작업 뒤에 수정·검토 작업을 추가할 수도 있다. 계획 변경 횟수와 새 DAG는 실행 화면에 반영된다.

## 작업자 보고

각 작업자의 프롬프트에는 실행별 보고 파일 경로가 포함된다. 보고 파일에 다음과 같은 JSON 한 줄을 추가한다.

```json
{"id":"dependency-changed-1","kind":"finding","message":"의존 API의 반환 필드가 바뀌었습니다. 후속 연결 작업의 입력을 수정해야 합니다."}
```

진행할 수 없는 문제는 `kind: "blocked"`로 보고하고 현재 시도를 마친다. 해당 시도는 CLI가 정상 종료해도 성공 산출물로 통합하지 않고 조율자에게 전달한다. 문제를 스스로 해결하며 진행할 수 있으면 `finding`을 사용한다. 보고마다 새 ID를 쓰며, 동일 보고의 재전송은 같은 ID와 내용을 사용한다.

파일은 SwitchBite가 실행별로 만들고 수집한다. 종료 후 보고 원문은 실행 기록에 보존하고, 보고용 파일은 프로젝트 결과에 포함하지 않는다. 일반 stdout/stderr는 실행 출력으로 유지하며 명시적인 보고만 조율 입력으로 사용한다.

## CLI와 MCP

CLI:

```text
switchbite workflow report RUN --request-file report.json
```

`report.json`:

```json
{"id":"user-direction-1","source":"user","task_id":"review","kind":"request_change","message":"최종 검토에서 새 완료 조건도 확인해 주세요."}
```

MCP의 `switchbite_report`는 `{run_id, report}`를 받는다. 기존 `switchbite_next`가 `replan`을 반환하면 조율자가 다음 결과를 `switchbite_observe`로 전달한다.

```json
{"plan":null,"reply":"현재 계획을 유지하고 보고한 조건을 확인하겠습니다.","interrupt_tasks":[]}
```

계획을 바꾸려면 `plan`에 전체 노드 목록을 넣는다. 완료·실행 중인 노드는 현재 정의를 보존한다. 특정 실행을 멈추려면 `interrupt_tasks`에 작업 ID를 넣는다. 계획 추가와 중단 요청을 함께 전달할 수 있으며, 중단 대상의 실행 상태는 코어가 최종 확인한다. 웹 앱에서는 이 조율 과정을 선택한 native CLI로 자동 실행한다.

## 구현 선택

SwitchBite의 기존 영속 이벤트·계획 검증·복구·프로세스 소유권 코드를 확장했다. 새 실행 프레임워크나 서버 설정을 추가하지 않았다.

설계 비교에는 [OpenHands의 대화 이벤트 API](https://github.com/openhands/software-agent-sdk/blob/main/openhands-agent-server/openhands/agent_server/README.md), [LangGraph의 checkpoint와 interrupt](https://docs.langchain.com/oss/python/langgraph/interrupts), [AutoGen의 handoff와 외부 종료](https://microsoft.github.io/autogen/dev/user-guide/agentchat-user-guide/tutorial/termination.html)를 확인했다. 외부 입력을 실행 상태에 기록하고 다음 조율 판단으로 연결하는 방식을 참고했으며, SwitchBite가 이미 소유한 Rust 실행 경로를 재사용했다.
