---
name: SwitchBite
description: 로컬 작업 실행을 관찰하고 조작하는 Work in motion 화면
colors:
  background: "#121314"
  surface: "#1a1c1e"
  surface-raised: "#202326"
  line: "#34383c"
  line-strong: "#4b5056"
  text: "#ecebe6"
  text-secondary: "#bdbab2"
  text-muted: "#95928a"
  accent: "#f0b429"
  accent-text: "#1a1408"
  focus: "#ffd166"
  running: "#7cb8ff"
  completed: "#7fd1a8"
  waiting: "#a9a49b"
  hold: "#c3a6ff"
  failed: "#ff8a80"
  paused: "#d8d3c7"
typography:
  body:
    fontFamily: '"Pretendard Variable", Pretendard, "Apple SD Gothic Neo", "Malgun Gothic", "Noto Sans KR", system-ui, sans-serif'
    fontSize: "0.875rem"
    fontWeight: 400
    lineHeight: 1.5
  title:
    fontFamily: '"Pretendard Variable", Pretendard, "Apple SD Gothic Neo", "Malgun Gothic", "Noto Sans KR", system-ui, sans-serif'
    fontSize: "1rem"
    fontWeight: 600
    lineHeight: 1.35
  label:
    fontFamily: '"Pretendard Variable", Pretendard, "Apple SD Gothic Neo", "Malgun Gothic", "Noto Sans KR", system-ui, sans-serif'
    fontSize: "0.8125rem"
    fontWeight: 600
    lineHeight: 1.35
  mono:
    fontFamily: '"D2Coding", "Cascadia Mono", ui-monospace, Consolas, "Noto Sans Mono CJK KR", monospace'
    fontSize: "0.8125rem"
    fontWeight: 400
    lineHeight: 1.45
rounded:
  compact: "4px"
  default: "6px"
  pill: "999px"
spacing:
  1: "4px"
  2: "8px"
  3: "12px"
  4: "16px"
  5: "24px"
  6: "32px"
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-text}"
    rounded: "{rounded.default}"
    padding: "6px 14px"
    height: "44px"
  button-secondary:
    backgroundColor: "transparent"
    textColor: "{colors.text}"
    rounded: "{rounded.default}"
    padding: "6px 14px"
    height: "36px"
  input:
    backgroundColor: "{colors.background}"
    textColor: "{colors.text}"
    rounded: "{rounded.default}"
    padding: "8px 10px"
    height: "40px"
  panel:
    backgroundColor: "{colors.surface}"
    rounded: "{rounded.default}"
    padding: "16px"
  status:
    rounded: "{rounded.pill}"
    padding: "2px 10px"
---

# Design System: SwitchBite

## Overview

**Creative North Star: "야간 관제 보드"**

승인된 Work in motion 목업의 어두운 운영 화면을 따른다. 설정과 조율, 작업 흐름과 실행 출력을 가까이 배치하고 실제 상태를 읽을 수 있게 한다. 시각 기준은 구현된 [스타일](assets/workflow-ui/styles.css)이며 화면별 동작은 [워크플로 계약](docs/design/workflow.md)을 따른다.

**Key Characteristics:**
- 어두운 면과 가는 경계선으로 구획한다.
- 노란색은 주요 실행과 선택에, 상태색은 실행 의미에 쓴다.
- 경로와 출력은 고정폭 글꼴로 표시한다.
- 상태는 색과 텍스트, 테두리 모양을 함께 사용한다.

## Colors

### Primary
- **앰버 강조** (`accent`): 작업 시작, 선택, 체크박스와 포커스 계열에 쓴다. 포커스는 별도 `focus` 색이다.

### Secondary
- **상태색**: 실행은 `running`, 완료는 `completed`, 대기는 `waiting`, 판단 대기는 `hold`, 실패는 `failed`, 일시정지는 `paused`를 쓴다.

### Neutral
- **어두운 바탕과 패널**: `background`, `surface`, `surface-raised`로 층을 나눈다. `line`과 `line-strong`은 경계에 쓴다.
- **밝은 글자**: `text`, `text-secondary`, `text-muted`로 정보의 위계를 구분한다.

## Typography

한글 본문은 로컬 Pretendard 계열과 시스템 대체 글꼴을 사용한다. 경로, 모델명과 로그는 D2Coding 계열 고정폭 글꼴을 사용한다. 기본 본문은 0.875rem, 보조 정보는 0.8125rem, 작은 메타 정보는 0.75rem이다. 패널 제목은 1rem과 600 굵기로 구분하며 숫자는 표 형식 폭을 사용한다. 긴 경로와 출력은 영역 안에서 줄바꿈한다.

## Layout

데스크톱은 340px 설정 레일과 나머지 작업 영역의 2열 구성이다. 바깥 여백은 24px, 패널 간격은 16px이다. 1279px 이하에서 레일은 300px이 되고, 959px 이하에서는 한 열로 바뀐다. 결과가 있으면 먼저 표시하고 설정, 조율자, 그래프, 작업자, 모델 풀 순으로 이어진다. 639px 이하에서는 바깥 여백 16px, 주요 조작 높이 최소 44px을 사용한다. 그래프와 로그는 자체 영역에서 스크롤한다.

## Elevation & Depth

기본 화면은 그림자보다 면 색과 테두리로 깊이를 만든다. 선택한 시도에는 앰버색 내부 하단 선, 실행 중 그래프 노드에는 파란 내부 선을 사용한다.

## Shapes

패널과 컨트롤은 작은 둥근 모서리, 상태 및 모델 표시는 알약형을 쓴다. 대기·판단 대기·실패는 실선 외에도 점선, 굵기와 색으로 구분한다. 조율자 패널은 이중 테두리를 사용한다.

## Components

### Buttons

주요 버튼은 앰버 바탕과 어두운 글자로 전체 너비를 차지한다. 보조 버튼은 투명 바탕과 선을 사용한다. 호버와 누름 상태에서 바탕색이 바뀌고 비활성 상태는 흐린 글자와 점선으로 표시한다. 키보드 포커스는 2px 윤곽선과 2px 간격으로 표시한다.

### Inputs / Fields

필드에는 어두운 바탕, 강한 경계선과 라벨을 사용한다. 일반 입력 높이는 최소 40px, 목표 입력은 최소 96px이다. 경로는 고정폭 글꼴이며 오류는 빨간 테두리와 메시지로 알린다.

### Cards / Containers

패널은 16px 안쪽 여백을 사용한다. 작업자 카드는 배정, 상태와 최근 출력을 요약하고 펼치면 시도별 로그와 조작을 보여 준다. 선택 상태는 앰버 경계로 표시한다.

### Status / Chips

상태 배지는 알약형 경계와 텍스트를 함께 쓴다. 모델 칩은 고정폭 글꼴이며 긴 이름은 줄바꿈한다. 시도 선택은 앰버 경계와 내부 선으로 표시한다.

### Workflow graph / Output

그래프 카드는 작업자 카드와 같은 상태 의미를 따르며 선택 시 앰버색으로 바뀐다. 카드를 끌어 배치하고 포트 사이를 연결하며 선택한 연결을 삭제할 수 있다. 역할은 카드 안에서 입력하고 자동 정렬과 화면 맞춤·확대 축소로 보기를 조정한다. 의존 관계는 선의 색과 점선으로 구분하고 선택한 선은 강조한다. 로그는 어두운 별도 영역에 고정폭으로 표시하고 오류 출력은 빨간 점선으로 구분한다. 실행 중 카드의 움직이는 선은 실제 상태에 연결하며 reduced-motion에서는 정적 패턴으로 바뀐다.

## Do's and Don'ts

### Do:
- **Do** 실제 상태를 텍스트와 색 및 경계 모양으로 함께 전달한다.
- **Do** 긴 경로와 로그를 해당 영역 안에서 읽을 수 있게 한다.
- **Do** reduced-motion 설정을 존중한다.

### Don't:
- **Don't** 실제로 알 수 없는 실행 정보를 추정해 표시한다.
- **Don't** 승인된 목업의 시각 체계를 임의로 교체한다.
