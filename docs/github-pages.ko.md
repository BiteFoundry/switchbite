# GitHub Pages

주소: https://bitefoundry.github.io/switchbite/

공개 페이지는 Work in motion UI에 작성된 예시 작업과 출력을 표시한다. 작업 카드와 도표를 선택해 병렬 의존성과 출력을 살펴볼 수 있다. 로컬 실행 기록, API 키, 세션 토큰은 게시하지 않는다.

실제 업무는 `switchbite workflow app --port 58431`로 실행한 로컬 앱에서 맡긴다. 공개 페이지의 **내 PC의 앱 열기** 버튼으로 이동한다. 공개 페이지에서 CLI를 실행하지 않는다.

## 게시

`node tools/build-pages.mjs <출력 디렉터리>`는 현재 UI와 `site/preview.js`의 예시 데이터만 복사한다. 출력은 GitHub Pages의 `gh-pages` 브랜치 루트에 게시한다. `build.json`에서 UI 소스 커밋을 확인할 수 있다.

검증: `node --test tests/pages_preview.mjs tests/workflow_ui_model.mjs`.

## 2026-10-28 게시 확인

- UI 소스: `9a70870e1fe595695cf050794b7a73e326d78ed3`
- Pages 커밋: `1790d24ab55e54f22b5e18176245adc0ac3c340d`
- [배포 실행](https://github.com/BiteFoundry/switchbite/actions/runs/40634371113): build/deploy 성공, Pages API `built`.
- 공개 `build.json`의 소스 커밋 일치. Aside에서 페이지 로드, 병렬 분기와 카드별 출력 확인. Node 테스트 7개 통과.

## preview.4 게시 확인

- UI 소스: `9832b0b8f2faf8505617750dce3de1386aeb059f`.
- Pages 커밋: `c99f565cc7b59aedb02f8641307f83a36e38a831`.
- [배포 실행](https://github.com/BiteFoundry/switchbite/actions/runs/40647800636): 성공. Pages API `built`, 공개 `build.json`의 소스 커밋 일치.
- Aside에서 공개 미리보기 로드를 확인했다. Node 테스트 9개 통과. 공개 페이지는 작성된 예시 데이터만 사용한다.
- [Windows preview.4](https://github.com/BiteFoundry/switchbite/releases/tag/v0.9.0-preview.4)와 자동 업데이트 채널도 게시했다. 공개 채널에서 새 디렉터리에 설치해 버전·실행 파일 해시·프로토콜을 확인했다.
