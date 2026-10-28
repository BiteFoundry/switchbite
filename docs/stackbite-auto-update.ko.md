# StackBite 최신 릴리스 자동 연동

SwitchBite가 StackBite 공식 릴리스를 확인하고, 그 릴리스의 `provider-stream`을 사용하는 연결 엔진을 자체 저장 공간에서 빌드한다. StackBite 저장소의 소스·Git 브랜치·설정·설치 파일·실행 중인 세션은 변경하지 않는다.

## 사용

```powershell
switchbite runtime follow-stackbite --source C:\Users\user\source\WorkStackBite-merge\WorkBite
switchbite workflow app
```

처음 설정하면 최신 릴리스를 확인하고 엔진 빌드·검증까지 진행한다. 이후 웹 앱 또는 MCP 서버(`serve`)가 실행 중인 동안 기본 60초 간격으로 확인한다. 변경 감지부터 적용까지는 확인 간격과 빌드 시간이 든다. 새 작업은 준비된 최신 엔진을 선택하고, 실행 중인 작업과 재개하는 작업은 저장된 실행 파일을 계속 사용한다.

```powershell
switchbite runtime stackbite-status
switchbite runtime check-stackbite
switchbite runtime follow-stackbite --source C:\path\to\source --interval-seconds 60
```

웹 상단에는 확인·소스 읽기·빌드·검증 상태, 공식 최신 버전과 새 작업에 사용할 버전이 표시된다. 작업에 이미 배정된 엔진 버전은 기존 작업 정보에 따로 표시된다. `--offline`에서는 자동 확인을 멈추고 준비된 엔진을 사용한다. `SWITCHBITE_STACKBITE_ENGINE`으로 명시한 엔진은 자동 선택보다 우선한다.

## 소스에 쓰지 않는 빌드

1. 공식 `Shinick-Han/stackbite/releases/latest`의 리다이렉트로 새 태그를 감지한다. 같은 태그는 저장된 릴리스 정보를 사용하므로 GitHub API를 매번 호출하지 않는다.
2. 해당 릴리스의 실제 배포 파일과 `.signature.json` 메타데이터에서 소스 커밋을 읽는다. 메타데이터 파일의 SHA-256을 GitHub 자산 digest와 비교하고, 버전·커밋·배포 파일 digest의 일치를 확인한다. 공개 태그의 오래된 `packaging/npm/release.json`은 기준으로 쓰지 않는다. 이 단계는 GitHub를 신뢰하는 메타데이터 일치 검사이며, Ed25519 서명의 독립 검증은 수행하지 않는다.
3. 지정한 소스 저장소에서 해당 커밋을 SwitchBite 전용 bare 저장소로 가져온다. 로컬에 커밋이 없으면 소스 저장소의 `origin` 주소를 읽어 같은 전용 캐시에서 가져온다. 원본 저장소에서 `fetch`, `checkout`, 파일 수정 또는 빌드를 실행하지 않는다.
4. 커밋의 Rust 소스를 SwitchBite 저장 공간으로 내보낸다. SwitchBite의 연결 어댑터는 별도 Cargo 프로젝트이며, 내보낸 `provider-stream`을 수정 없이 path dependency로 사용한다.
5. 어댑터 테스트와 실제 자식 프로세스의 stdin·stdout·stderr·EOF·종료 코드 검사를 통과하면 버전별 실행 파일을 보관하고 새 작업용 포인터를 교체한다. 실패하면 이전 준비 엔진을 유지한다. 여러 웹 앱이 열려 있어도 파일 잠금과 최근 확인 시각을 공유한다.

소스 저장소의 읽기 권한, Git, Rust/Cargo가 필요하다. 비공개 원격 저장소를 읽을 때는 기존 Git 인증을 사용한다. SwitchBite는 제공자 로그인이나 Git 인증 설정을 바꾸지 않는다. 자동 연동 설정이 가리키는 소스가 보관 상태(`ARCHIVED.json`)면 복원 전까지 빌드를 진행하지 않는다.

동기화 대상은 현재 SwitchBite가 쓰는 **StackBite의 CLI 실행 컴포넌트**다. StackBite GUI, Room·Seat 제어 API의 새 기능을 자동으로 추가하는 기능은 별도 연결 작업이 필요하다.

## 저장 위치

모든 생성 파일은 SwitchBite 런타임 저장소 아래에 둔다. 기본 경로는 `%LOCALAPPDATA%\Switchbite\runtime`이며 `SWITCHBITE_RUNTIME_HOME`으로 바꿀 수 있다. 이 경로가 원본 소스 저장소 안에 있으면 설정을 거부한다.

| 경로 | 내용 |
|---|---|
| `stackbite-follow.json` | 소스 저장소와 확인 간격 |
| `stackbite-update.json` | 최근 확인·빌드 상태 |
| `stackbite-current.json` | 새 작업에 쓸 엔진의 버전·커밋·해시 |
| `stackbite-source.git` | SwitchBite 전용 소스 캐시 |
| `stackbite-builds/<버전-커밋-어댑터>/` | 소스 스냅샷, 어댑터, `build.log` |
| `stackbite-build-target/` | 반복 빌드에서 재사용하는 Cargo 출력 |
| `stackbite-engines/<버전-커밋-어댑터>/` | 작업별로 고정해 쓰는 실행 파일 |

StackBite 구성 요소의 `LicenseRef-Proprietary` 표기는 유지한다. 이 기능은 SwitchBite에서만 구현한다.
