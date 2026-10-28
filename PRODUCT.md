# SwitchBite

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

개인 PC에 로그인된 AI CLI들을 사용해 프로젝트를 진행하는 사용자와 작업을 지휘하는 조율자.

## Product Purpose

사용자가 도구를 고르고 프로젝트 경로와 업무를 입력하면 작업을 분해·배정하고, 병렬 진행·실행 출력·완료 결과를 한 화면에서 확인한다.

## Operating Context

Rust 실행 파일이 로컬 웹 앱과 API를 제공한다. 로그인된 Claude, Codex, Cursor, OpenCode, Antigravity CLI로 새 작업 세션을 실행한다. 도구의 실행 지원 여부와 모델 목록은 실제 탐색 결과를 표시한다. Jev 또는 Laya 판단 설정은 별도로 유지한다.

## Capabilities and Constraints

- StackBite 원본 소스, 설치와 실행 세션을 변경하지 않는다. SwitchBite 전용 엔진과 캐시를 사용한다.
- Python 런타임을 추가하지 않는다. HTML/CSS/ES 모듈을 기존 Rust 서버에서 제공한다.
- 작업 의존 관계, 실행 상태, 요청한 모델과 실제 보고된 모델, 출력은 서버의 기록을 따른다.
- 사용자에게 재개가 필요한 상태와 생성된 결과물을 찾을 수 있는 경로를 표시한다.

## Brand Commitments

2026-10-28 사용자는 `Work-in-motion-mockup-2026-10-28` HTML 목업을 실제 SwitchBite 웹 UI로 채택하도록 승인했다. 목업의 구성과 시각 체계를 재사용하며 가상 시나리오를 실제 API 연결로 바꾼다.

## Product Principles

- 도구 선택 → 업무 입력 → 실행 관찰 → 결과 확인을 이어 준다.
- 자주 쓰는 조작을 화면 가까이에 둔다.
- 실제로 병렬 실행되는 작업과 대기 원인을 구별한다.
- 모르는 실행 정보를 추정값으로 채우지 않는다.
