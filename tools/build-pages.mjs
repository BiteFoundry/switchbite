// Build only the public UI and authored example data. No live runtime files are copied.
import { mkdir, readFile, copyFile, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { execFileSync } from 'node:child_process';

const root = resolve(import.meta.dirname, '..');
const output = resolve(process.argv[2] || join(root, '.switchbite/pages-site'));
await mkdir(join(output, 'ui'), { recursive: true });
for (const file of ['styles.css', 'graph-output.css', 'live.css', 'app.js', 'model.js', 'graph.js', 'worker-output.js']) {
  await copyFile(join(root, 'assets/workflow-ui', file), join(output, 'ui', file));
}
await copyFile(join(root, 'site/preview.js'), join(output, 'preview.js'));
let html = await readFile(join(root, 'assets/workflow.html'), 'utf8');
html = html.replaceAll('href="/ui/', 'href="./ui/');
html = html.replace(/<script>const token='__SESSION_TOKEN__';[^<]+<\/script><script type="module" src="\/ui\/app.js"><\/script>/,
  `<script type="module">import {api,info} from './preview.js';globalThis.switchbite={token:'',appMode:false,publicPreview:true,publicApi:api,publicInfo:info};await import('./ui/app.js');</script>`);
html = html.replace('<title>Workflow · Switchbite</title>', '<title>SwitchBite · Work in motion</title>');
html = html.replace('<meta name="viewport"', '<meta name="description" content="SwitchBite — AI 도구 선택, 작업 분해와 모델 배정, 병렬 진행과 실행 출력을 한 화면에서 확인합니다."><meta name="viewport"');
html = html.replace('<div class="wim-banner">', '<section class="public-entry" aria-label="SwitchBite 사용"><p>도구를 고르고, 업무를 맡기고, 병렬 진행을 확인하세요.</p><div><a class="wim-btn wim-btn--primary" href="http://127.0.0.1:58431/" rel="noopener noreferrer">내 PC의 앱 열기</a><a class="wim-btn" href="https://github.com/BiteFoundry/switchbite/tree/feature/automatic-model-pool" target="_blank" rel="noopener noreferrer">코드·사용법</a></div><small>로컬 실행: <code>switchbite workflow app --port 58431</code></small></section><div class="wim-banner">');
html = html.replace('</head>', `<style>.public-entry{padding:20px var(--wim-pad);border-bottom:1px solid var(--wim-line);display:flex;gap:12px 24px;align-items:center;flex-wrap:wrap}.public-entry p{flex:1;font-size:1rem;font-weight:600}.public-entry div{display:flex;gap:8px;flex-wrap:wrap}.public-entry a{text-decoration:none;display:inline-flex;align-items:center}.public-entry small{flex-basis:100%;color:var(--wim-ink-2)}.public-entry code{overflow-wrap:anywhere}.wim-connection[data-connected="preview"]{color:var(--wim-accent)}@media(max-width:639px){.public-entry{align-items:stretch}.public-entry div,.public-entry a{width:100%}.public-entry a{justify-content:center}}</style></head>`);
if (html.includes('__SESSION_TOKEN__') || html.includes('__APP_MODE__') || html.includes('src="/ui/')) throw Error('Unresolved private/local configuration');
await writeFile(join(output, 'index.html'), html);
await writeFile(join(output, '.nojekyll'), '');
await writeFile(join(output, 'build.json'), JSON.stringify({ source_commit: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim(), mode: 'public-preview', runtime_data: 'authored-examples-only' }, null, 2));
console.log(JSON.stringify({ output, mode: 'public-preview', files: 11 }));
