// Authored public examples only. No local run records, prompts, paths or credentials are loaded.
export const info = {
  app_version: '0.9.0-preview.2', workspace: 'C:\\Projects\\sample-app', judge: '예시 모델 풀',
  selected_tools: ['codex', 'claude', 'antigravity'],
  tools: [
    { id: 'codex', name: 'Codex', selectable: true, model_count: 1, status: { label: '예시 프로필' } },
    { id: 'claude', name: 'Claude', selectable: true, model_count: 1, status: { label: '예시 프로필' } },
    { id: 'antigravity', name: 'Antigravity', selectable: true, model_count: 1, status: { label: '예시 프로필' } },
  ],
  coordinators: [{ id: 'coordinator', provider: 'codex', model: 'gpt-6.1-terra', effort: 'high', label: 'Codex / gpt-6.1-terra / high' }],
};
const task = (id, objective, role, status, depends_on, provider, model, effort) => ({ id, objective, role, status, depends_on,
  provider, model, effort, outputs: [], observed: {}, evidence: [] });
export const state = {
  run_id: 'public-example', status: 'running', workspace: info.workspace,
  goal: '프로젝트 화면과 API를 병렬로 구현하고, 결과를 검증한 뒤 검토합니다.',
  tools: info.selected_tools, max_parallel: 3, progress: { done: 1, total: 5 },
  coordinator: { provider: 'codex', model: 'gpt-6.1-terra', effort: 'high', status: 'coordinating' },
  tasks: [
    task('plan', '요구사항을 정리하고 독립 작업과 의존 관계를 나눕니다.', 'planning', 'done', [], 'codex', 'gpt-6.1-terra', 'high'),
    task('interface', '사용자가 바로 살펴볼 수 있는 프로젝트 화면을 만듭니다.', 'mockup', 'running', ['plan'], 'antigravity', 'claude-sonnet-5-5-medium', 'default'),
    task('implementation', '화면과 연결할 API 및 데이터 처리를 구현합니다.', 'implementation', 'running', ['plan'], 'claude', 'opus', 'high'),
    task('verification', '화면과 API의 연결 및 동작을 검증합니다.', 'testing', 'pending', ['interface', 'implementation'], '', '', ''),
    task('review', '변경 사항과 검증 결과를 확인하고 결과물을 정리합니다.', 'review', 'pending', ['verification'], '', '', ''),
  ],
};
const exampleStreams = [
  { id: 'example-coordinator', task_id: null, stage: 'plan', provider: 'codex', status: 'completed', health: 'completed', process_alive: false,
    elapsed_ms: 8000, recent_output: '화면 작업과 API 작업은 독립적으로 진행할 수 있습니다.', lines: ['요구사항을 작업 단위로 나누었습니다.', '화면과 API를 병렬로 배정하고, 두 작업 이후 검증과 검토를 연결했습니다.'] },
  { id: 'example-plan', task_id: 'plan', stage: 'worker', provider: 'codex', status: 'completed', health: 'completed', process_alive: false,
    elapsed_ms: 8000, recent_output: '작업 5개와 의존 관계를 정리했습니다.', lines: ['계획을 정리했습니다.', '독립 작업: interface, implementation'] },
  { id: 'example-interface', task_id: 'interface', stage: 'worker', provider: 'antigravity', status: 'running', health: 'active', process_alive: true,
    elapsed_ms: 16000, recent_output: '작업자 카드와 그래프를 화면에 배치하고 있습니다.', lines: ['프로젝트 화면 구성을 확인했습니다.', '작업자 카드와 그래프를 화면에 배치하고 있습니다.'] },
  { id: 'example-implementation', task_id: 'implementation', stage: 'worker', provider: 'claude', status: 'running', health: 'active', process_alive: true,
    elapsed_ms: 17000, recent_output: '작업 상태와 출력 데이터를 전달하는 API를 구현하고 있습니다.', lines: ['기존 API 계약과 데이터 흐름을 확인했습니다.', '작업 상태와 출력 데이터를 전달하는 API를 구현하고 있습니다.'] },
];
const streams = exampleStreams.map(stream => ({ ...stream, can_retry: false, last_output_at_ms: null,
  entries: stream.lines.map((text, index) => ({ id: `${stream.id}:${index}`, source: 'stdout', text })) }));
export async function api(path, body) {
  if (body !== undefined) throw Error('실제 업무는 로컬 앱에서 실행하세요.');
  if (path === '/api/state') return structuredClone(state);
  if (path.startsWith('/api/activity')) {
    const id = new URL(path, 'https://preview.invalid').searchParams.get('stream');
    const selected = streams.find(stream => stream.id === id) || streams.find(stream => stream.process_alive);
    return structuredClone({ run_id: state.run_id, health: { status: 'active' }, streams, selected_stream: selected });
  }
  throw Error('공개 미리보기에서 제공하지 않는 요청입니다.');
}
