# 자동 모델 풀 사용

`switchbite codex` 또는 `switchbite claude`로 한 번 연결한 뒤 새 조율 세션에서 프로젝트 목표를 전달한다. 판단기는 기존 Jev/Laya 설정을 사용한다. 일반 연결은 실행마다 전달한 workspace에 기록을 저장하며, 프로젝트 단위 `switchbite stackbite` 연결은 지정한 workspace를 사용한다.

`switchbite_start`의 pool을 생략하면 설치된 Claude Code·Codex·Cursor Agent·OpenCode에서 모델 목록을 구성한다. 명시한 pool은 수동 모드로 사용한다. `switchbite models`로 목록을 보고 `switchbite models --refresh`로 새로 조회할 수 있다.

```json
{
  "workspace": "C:\\project",
  "catalog": { "providers": ["claude", "codex", "cursor", "opencode"] },
  "plan": {
    "nodes": [
      {
        "id": "implement",
        "objective": "요청한 JSON 설정 파일을 구현한다",
        "role": "implementation",
        "acceptance": ["설정이 JSON으로 파싱되고 명세의 값을 포함한다"],
        "outputs": ["config.json"],
        "write_paths": ["config.json"]
      }
    ]
  }
}
```

조율자는 next를 한 번 호출해 실행을 시작하고 status로 진행을 확인한다. 작업마다 전체 목록에서 최대 16개 후보를 추린다. 설정한 모델·effort와 CLI가 보고한 우선순위, 역할 선호와 후보 다양성을 사용한다. 수락 조건과 산출물, 선행 작업의 통합 결과를 판단기에 전달한다.

정확한 대상을 선택하려면 해당 노드에 `routing`을 넣는다. 지정한 필드는 함께 적용한다.

```json
{ "routing": { "provider": "claude", "model": "opus", "effort": "medium" } }
```

`candidate_id`로 모델·effort·실행 자원을 한 번에 지정할 수도 있다. 지정 대상이 없으면 모델 목록과 연결 상태를 확인하도록 반환한다. 실행 도중 provider나 effort를 바꾸지 않는다.

지정 조건에 맞는 대상이 하나이면 그 대상을 실행한다. 여러 후보를 비교하는 자동 배정은 Jev/Laya가 판단한다. 두 경로 모두 같은 산출물 검사와 통합을 사용한다.

Codex는 설치된 모델 캐시와 사용자 설정을 읽고, Claude는 확인한 모델 별칭·사용자 설정과 CLI의 effort 옵션을 사용한다. Cursor와 OpenCode는 CLI 모델 조회를 사용한다. catalog의 readiness가 unverified이면 인증된 실제 실행은 아직 확인하지 않은 상태다. 실행기는 원래 CLI의 로그인과 모델 접근 권한을 사용한다.

기본 역할 선호는 Codex=조율·리뷰, Claude=구현, Antigravity=목업·작문, Cursor·Grok=균형형이다. 이번 자동 네이티브 실행 대상은 Claude·Codex·Cursor·OpenCode다. 요청의 preferences로 선호를 조정한다.

모델 목록은 실행 시도와 별도로 저장한다. 각 배정은 후보 snapshot을 보관하고, 이미 시작한 작업은 그 설정으로 마무리한다. 연결을 해결한 뒤 next 또는 CLI continue로 이어가면 대기 중인 자동 모델 풀을 다시 조회한다. 이전 수동 pool 실행 기록도 같은 도구로 확인할 수 있다.
