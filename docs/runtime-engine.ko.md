# SwitchBite 런타임 엔진

이 구성은 SwitchBite `0.9.0-preview.1` 웹 앱과 독립된 StackBite 복제본 기반 엔진을 함께 사용한다. 복제본의 StackBite 앱 기준 버전은 `0.5.54`, 브리지 버전은 `0.1.0`, 브리지 프로토콜은 `1`이다. 기존 StackBite 설치와 그 상태는 변경하지 않는다.

SwitchBite가 준비한 작업 디렉터리에서 엔진 프로세스를 실행하고, 엔진은 기존 `stackbite-provider-stream`을 통해 제공자 CLI의 출력과 오류 출력을 전달한다. SwitchBite는 기존 프로세스 기록, 취소, 출력 제한과 제공자별 완료 검증을 사용한다. 별도 데몬이나 SSE 서버는 필요하지 않다. 브리지는 Codex, Claude, Cursor, OpenCode, Antigravity의 기존 실행 어댑터를 지원한다. Grok 실행 어댑터는 포함하지 않는다.

## 런타임 채널

다음 명령은 지정한 매니페스트를 읽고 SwitchBite와 StackBite 엔진 실행 파일을 한 번들로 설치한다. 실제 매니페스트의 절대 경로나 HTTPS URL을 사용한다.

```powershell
switchbite runtime configure --manifest <매니페스트_절대경로_또는_HTTPS_URL>
switchbite runtime status
switchbite runtime sync
switchbite workflow app
```

`configure`는 매니페스트의 두 파일을 설치한 뒤 채널을 저장한다. `status`는 현재 설치된 번들 정보를 보여 주며, `sync`는 설정된 채널을 다시 확인한다. 설정된 채널이 없으면 상태는 `unconfigured`다. 새 웹 작업을 시작할 때도 설정된 채널을 확인하고, 필요한 경우 새 번들의 앱으로 작업 요청을 넘긴다. 실행 중인 작업은 시작할 때 기록된 런타임과 프로젝트 기준 커밋을 사용한다. `--offline` 실행은 채널 동기화를 건너뛴다.

여기서 최신 버전은 **설정된 채널의 매니페스트가 가리키는 번들**을 뜻한다. 별도로 채널을 제공하거나 설정하지 않으면 업스트림 릴리스를 자동으로 찾지 않는다.

## 매니페스트 형식

아래 값은 형식을 보여 주는 자리표시자다. 배포할 때 실제 실행 파일 위치와 각 파일의 SHA-256을 넣는다. 매니페스트와 각 파일의 `source`는 절대 파일 경로 또는 HTTPS URL이다.

```json
{
  "schema_version": 1,
  "protocol_version": 1,
  "bundle_id": "example-bundle-1",
  "switchbite": {
    "version": "0.9.0-preview.1",
    "source": "<SwitchBite 실행 파일 위치>",
    "sha256": "<SwitchBite 파일의 64자리 SHA-256>"
  },
  "stackbite": {
    "version": "0.1.0",
    "source": "<StackBite 브리지 실행 파일 위치>",
    "sha256": "<브리지 파일의 64자리 SHA-256>"
  }
}
```

설치 시 두 파일의 해시와 엔진 프로토콜을 확인한다. 동일한 `bundle_id`에 이미 설치된 파일의 해시가 다르면 충돌로 처리하므로, 파일이 바뀐 번들에는 새 ID를 사용한다. 기본 런타임 저장 위치는 Windows의 `%LOCALAPPDATA%\Switchbite\runtime`이다. 격리 테스트가 필요할 때만 절대 경로의 `SWITCHBITE_RUNTIME_HOME` 환경 변수로 저장 위치를 바꿀 수 있다. 이 과정에서 제공자 인증 정보를 복사할 필요는 없다.
