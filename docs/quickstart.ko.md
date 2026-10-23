# Switchbite 사용 안내

이 문서는 v0.3 연결 기록입니다. v0.4의 새 기본 프로필과 사용 흐름은
[오케스트레이터 사용 안내](agent-experience.ko.md)를 먼저 읽으세요.

Switchbite는 작업과 후보 풀을 받아 적절한 작업자를 추천하는 Rust MCP 플러그인입니다.
현재 판단 모델은 Jev 1.14이며, 실제 작업을 수행할 후보 모델과는 별개입니다.
첫 공개 버전은 Windows x64용 v0.3.0 프리뷰입니다.

## CLI 연결

릴리스 ZIP과 SHA256SUMS를 다운로드해 해시를 확인하고 ZIP을 풉니다.

```powershell
.\bin\switchbite.exe --version
.\bin\switchbite.exe config --client claude
.\bin\switchbite.exe config --client codex
.\bin\switchbite.exe config --client cursor
.\bin\switchbite.exe config --client grok
.\bin\switchbite.exe config --client antigravity
.\bin\switchbite.exe config --client opencode
```

명령은 설정 조각만 출력합니다. 기존 MCP 설정에 병합한 뒤 새 세션을 시작합니다.
전체 설정 파일을 덮어쓰지 마세요. 온라인 판단에는 MCP 자식 프로세스에 전달한
`TYPESAFE_API_KEY`가 필요합니다. API 이용 조건은 MIT 소프트웨어 라이선스와 별개입니다.
`config`에 `--offline`을 추가하면 상위 환경에 키가 있어도 온라인 판단을 하지 않습니다.

| client | 설정 구조 |
| --- | --- |
| Claude | `.mcp.json`의 `mcpServers.switchbite`, 또는 `--mcp-config FILE` |
| Codex | `~/.codex/config.toml`의 `[mcp_servers.switchbite]` |
| Cursor | workspace `.cursor/mcp.json`의 `mcpServers.switchbite` |
| Grok | `.grok/config.toml`의 `[mcp_servers.switchbite]` |
| Antigravity | workspace `.agents/mcp_config.json`의 `mcpServers.switchbite` |
| OpenCode 2 | 유효한 설정의 `mcp.servers.switchbite` |

`switchbite_status`로 연결과 judge 설정 존재 여부를 확인합니다. 인증 성공이나
쿼터를 확인하는 도구는 아닙니다. OpenCode에서는 상위 설정이 프로젝트 설정을
덮을 수 있습니다. `switchbite.exe doctor --client opencode`는 해당 환경변수의
이름만 표시합니다. 기존 서버 승인·프로젝트 신뢰 절차는 클라이언트가 관리합니다.

## 오케스트레이터에서 쓰기

Herdr·Orca·Stackbite 등에서 사용하려면 coordinator의 MCP에 연결하거나 Rust
라이브러리/JSON CLI로 호출할 수 있습니다. 후보는 CLI 이름 하나가 아니라
`실행 도구 + 모델 + 설정 + 작업자 인스턴스` 조합별로 구분합니다.
호스트가 실제 후보 목록과 가용성·권한·쿼터 상태를 확인해 전달해야 합니다.

이미 구현된 것은 후보 풀 입력, 가용성·필수 역량 필터, 작업 적합도·사용자 선호
평가, 배정 제안 또는 보류입니다. 자동 모델 탐색, 비용·지연 수치 최적화,
전체 DAG 자원 배치, 작업 실행, 피드백 학습은 구현되어 있지 않습니다.
`extensions`는 호스트용 메타데이터이며 Jev에 전송하거나 점수에 반영하지 않습니다.

`ready=true`일 때 반환된 `candidate_id`를 원래 후보와 매핑하고, 현재 상태를
재검사한 뒤 호스트가 예약·실행합니다. `ready=false`에서는 진단용 추천값이 있어도
실행하지 않습니다. [모델 풀 설명](model-pools.md)에 전체 연결 절차가 있습니다.

이전 비공개 프로토타입의 `jev-router` 서버/실행 파일/도구 이름은 이번 제품에서
`switchbite`로 바뀌었습니다. 설정과 coordinator 프롬프트도 새 이름으로 변경해야
합니다. 기존 설치를 자동 수정하지 않으며 Stackbite 호스트 코드는 포함하지 않습니다.
