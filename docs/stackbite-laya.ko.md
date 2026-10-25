# Stackbite coordinator용 로컬 Laya 연결

이 MVP는 Stackbite가 프로젝트에서 시작하는 Codex/Claude coordinator CLI에
Switchbite MCP를 등록합니다. Stackbite 자체의 플러그인 레지스트리나 실행 경로를
수정하는 기능은 아닙니다. 원본 Stackbite 코드, 설치 파일, 런타임과 전역 CLI 설정은
변경하지 않습니다.

## 설치

이 브랜치 버전은 `0.6.0-laya.1`입니다. 원본 main의 Studio 작업과 독립된
`feature/laya-stackbite-plugin` 작업 트리에서 개발합니다.

로컬 Laya 서버를 준비하고 다음을 실행합니다. `--workspace`에는 coordinator를 시작할
프로젝트의 절대 경로를 지정합니다.

```powershell
switchbite stackbite --workspace C:\path\to\project --client both --endpoint http://127.0.0.1:11435 --model laya:multilingual --dry-run --json
switchbite stackbite --workspace C:\path\to\project --client both --endpoint http://127.0.0.1:11435 --model laya:multilingual --json
```

`--client codex`, `--client claude`, `--client both`를 지원합니다. 설치 대상은 다음과 같습니다.

- Codex: 프로젝트의 `.codex/config.toml`
- Claude Code: 프로젝트의 `.mcp.json`
- 연결 설명과 coordinator 지침: `.switchbite/stackbite-plugin.json`

서버 이름은 `switchbite`이며, 실행 인자는
`serve --profile agent --judge laya --endpoint ... --model ...`입니다.
실행 파일은 Switchbite의 콘텐츠 해시 기반 관리 디렉터리에 복사되므로 다운로드 ZIP이나
원래 실행 파일을 옮겨도 연결이 유지됩니다. PATH와 셸 프로필은 변경하지 않습니다.

기존 설정의 다른 서버, 사용자 모델 선택과 주석은 유지하며, 변경 전 바이트 그대로의
백업을 설정 옆에 남깁니다. `switchbite` 이름이 다른 서버에 사용 중이면 덮어쓰지 않습니다.
기존 Switchbite 등록을 Laya로 전환할 때는 해당 서버의 명시적인 `TYPESAFE_API_KEY`
환경 전달 항목을 제거합니다. 환경 변수의 실제 값을 읽어 설치 설정에 넣지 않습니다.
프로세스 환경 상속 자체는 coordinator CLI의 동작을 따릅니다.

설치 성공은 파일 등록을 뜻합니다. 기존 Claude 사용자 범위의 동명 `switchbite`
서버가 있으면 실제 적용 여부를 별도로 확인해야 합니다. 이번 로컬 검사에서도
Codex는 프로젝트 Laya 설정을 선택했지만, Claude의 `mcp get switchbite`는 기존
사용자 설정의 Jev 실행 인자를 보고했습니다. 기존 전역 설정을 자동 변경하지 않습니다.
새 세션의 MCP 승인·실효 설정을 확인하거나, 해당 세션에 명시적으로
`claude --strict-mcp-config --mcp-config C:\path\to\project\.mcp.json`을 지정하세요.
이 옵션은 해당 세션에서 다른 MCP 설정을 제외하므로 필요한 서버는 같은 파일에 명시합니다.

두 클라이언트를 선택하면 모두 사전 검증한 뒤 변경합니다. 이후 파일 잠금이나 권한 문제로
쓰기 일부가 실패하면 오류에 완료된 클라이언트를 표시합니다. 전체 롤백을 주장하지 않으며,
문제를 해결한 뒤 같은 명령을 다시 실행하면 됩니다. `--dry-run`은 파일, 디렉터리, 백업,
잠금 파일이나 실행 파일 복사본을 만들지 않습니다.

## coordinator에서 사용

설치 후 같은 프로젝트에서 **새 coordinator CLI 세션**을 시작합니다. Codex 프로젝트 신뢰
설정이나 Claude MCP 승인 안내가 나오면 사용자가 승인해야 합니다. 이미 실행 중인 세션은
등록을 자동으로 다시 읽는다고 가정하지 않습니다. 실제 세션에서 `switchbite` MCP 도구가
보이는지 확인합니다. 별도 CLI 옵션이나 상위 정책에 의한 설정 재정의는 설치 성공만으로
검증되지 않습니다.

설치는 자동 계획 수립이나 worker 실행 권한을 추가하지 않습니다. 다음 지침을 coordinator
요청에 포함할 수 있습니다. 이 설치기는 `AGENTS.md`나 `CLAUDE.md`에 자동으로 쓰지 않습니다.

> 작업 제약과 실제 worker/model inventory를 `switchbite_route`에 전달하고, 반환된 추천을
> 참고하여 등록된 worker ID로 작업을 위임하세요. hold이면 제약, inventory 또는 로컬 Laya
> 연결을 먼저 해결하세요. worker 실행과 결과 검증, 최종 사용자 답변 취합은 coordinator가
> 담당합니다.

로컬 Laya 응답 실패나 판단을 사용할 수 없는 경우에는 hold를 유지합니다. Jev로 조용히
대체하지 않습니다. 이 MVP의 판단은 추천 데이터이며, host가 사용 가능한 worker와 실행
정책을 기준으로 최종 위임을 결정합니다.

## 로컬 모델 준비와 점검

모델과 추론기는 플러그인 ZIP과 별도입니다. Python 없이 동작하는 Ollaya를 사용할 수
있습니다. 모델의 최초 반입은 인터넷이 가능한 준비 환경에서 수행하고, 보안 환경에는
승인된 바이너리·모델·토크나이저·라이선스·해시를 함께 반입합니다.

```powershell
# 터미널 1: 전용 모델 폴더와 루프백 주소로 서버 실행
$env:OLLAYA_HOST = '127.0.0.1:11435'
$env:OLLAYA_MODELS = 'C:\approved-models\switchbite-laya'
ollaya serve

# 준비 환경의 터미널 2: 최초 반입 시에만 실행
$env:OLLAYA_HOST = '127.0.0.1:11435'
ollaya pull laya:multilingual

# 플러그인 연결 점검: 합성 입력만 사용하며 worker는 실행하지 않음
switchbite probe --judge laya --endpoint http://127.0.0.1:11435 --model laya:multilingual
```

이미 승인된 모델이 있으면 pull 없이 서버를 실행합니다. 플러그인은 자동 다운로드·
외부 API 대체·서버 자동 시작을 하지 않습니다. 서버 재시작은 운영 환경에서 별도로
관리해야 하며, 선택한 서버가 없으면 `start_local_laya` 다음 행동을 반환합니다.
주소는 리터럴 루프백 IP의 HTTP만 허용합니다. 프록시·리다이렉트·Authorization 전송은
하지 않으며, 모델명은 응답과 정확히 일치해야 합니다.

API 형식 호환성과 확신도 의미는 구분합니다. 플러그인은 확률 분포를 검증하고 기존
정책에서 사용하는 순서형 Score 집중도를 계산합니다. 이는 실제 정답률 보정이나
도메인 학습이 아닙니다. 선호가 비어 있으면 Rust에서 중립으로 처리해 불필요한 모델
질문을 생략하고, 명시한 선호는 계속 모델로 전달합니다. 잘림·범위 오류·모델 불일치는
배정으로 바꾸지 않습니다.

## 2026-10-25 검증 범위

- Ollaya `0.17.0` Windows CPU 배포본 SHA256 검증 후 전용 폴더에서 실행.
- 실제 `laya:multilingual` 로컬 추론 응답 확인.
- 설치된 해시 기반 실행 파일로 MCP 초기화, 도구 4개 발견, 상태 모델명,
  실제 `switchbite_route`의 `ready / recheck_and_dispatch` 응답 확인.
- 현재 Jev-Router 작업 폴더의 Codex에서 프로젝트 Laya 설정 선택 확인.
- Claude에는 프로젝트 설정을 등록했으나 기존 사용자 서버가 실효 설정으로 관찰됨.
  프로토콜 검증은 명시적 프로젝트 설정의 설치 실행 파일을 사용한 MCP 클라이언트에서 수행.
- worker 실행, 실제 Claude/Codex 모델 호출, 자동 계획 생성, 학습·확률 보정,
  CUDA 성능, 폐쇄망 설치 및 전체 DAG 실행은 이번 MVP의 검증 범위에 포함하지 않음.

현재 프로비저닝은 원본 Stackbite 설치·기존 세션·전역 CLI 설정을 변경하지 않습니다.
WorkBite Task binding은 확인되지 않아 `.switchbite/laya-build`와 로컬 런타임은
StorageGuardian에 자동 등록하지 않고 보존했습니다.
