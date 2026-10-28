// View models derived only from recorded workflow and process state.
export const ACTIVE = new Set(['running', 'planning', 'paused']);
export const COPY = {
  replanning: '계획 조정 중', replan: '계획 조정', repairing_plan: '계획 형식 수정 중',
  idle: '새 작업 대기', planning: '계획 중', running: '실행 중', paused: '일시정지',
  completed: '완료', cancelled: '취소됨', blocked: '선행 작업 확인 필요',
  held: '배정 확인 필요', failed: '실패', unknown: '상태 확인 중',
  pending: '배정 대기', routed: '작업 준비 중', prepared: '실행 대기',
  produced: '결과 검사 중', verified: '통합 대기', done: '완료',
  verifying: '검증 중', integrating: '통합 중', needs_attention: '확인 필요',
  waiting_dependency: '선행 작업 대기', routing_hold: '배정 확인 필요', ready: '실행 대기',
  active: '진행 중', quiet: '새 출력 대기', silent: '한동안 새 출력 없음',
  needs_input: '입력·로그인 확인 필요', launching: '실행 시작 중', timeout: '시간 초과',
  plan: '계획', launch: '실행', verify: '검증', integrate: '통합', prepare: '준비',
  inspect: '상태 확인', review_route: '배정 검토', gather_context: '맥락 수집', repair: '재계획', worker: '실행',
};
export const ROLES = { implementation: '구현', review: '검토', exploration: '조사',
  design: '설계', planning: '계획', testing: '검증', writing: '작성', mockup: '목업', integration: '통합' };
export const label = value => COPY[value] || value || '대기';
export function feedbackView(report) {
  const source = report.source === 'user' ? '사용자' : report.source === 'worker' ? `작업자${report.task_id ? ` · ${report.task_id}` : ''}` : '실행 상태';
  const changes = [['added', '추가'], ['changed', '변경'], ['removed', '제거']].flatMap(([key, title]) => report.changes?.[key]?.length ? [`${title}: ${report.changes[key].join(', ')}`] : []);
  return { id: report.id, source, status: report.status === 'processed' ? report.kind === 'dependency_change' ? '반영 완료' : '답변 완료' : '조율 대기', message: report.message || '', reply: report.reply || '', changes: changes.join(' · ') };
}

export function dependencyChoices(tasks, taskId) {
  const descendants = new Set([taskId]);
  let changed = true;
  while (changed) {
    changed = false;
    for (const task of tasks) {
      if (!descendants.has(task.id) && (task.depends_on || []).some(id => descendants.has(id))) {
        descendants.add(task.id); changed = true;
      }
    }
  }
  return tasks.filter(task => !descendants.has(task.id));
}
export function failureText(problem) {
  if (!problem) return '';
  const stages = { plan_validation: '계획 검증', result_collection: '계획 회수', cli_execution: 'CLI 실행', routing: '모델 배정', workflow: '작업 조율', integration: '결과 통합' };
  const location = [problem.node_id, problem.field].filter(Boolean).join(' · ');
  return [stages[problem.stage], location, problem.message || problem.code, problem.expected ? `기대 형식: ${problem.expected}` : ''].filter(Boolean).join(' — ');
}
export const usableToolIds = (tools, selected) => (tools || []).filter(tool => tool.selectable && selected.has(tool.id)).map(tool => tool.id);
export const pathLabel = value => String(value || '').replace(/^\\\\\?\\/, '');
export function duration(ms) {
  if (ms == null) return '—';
  const seconds = Math.max(0, Math.floor(ms / 1000));
  return seconds >= 3600 ? `${Math.floor(seconds / 3600)}시간 ${Math.floor(seconds / 60) % 60}분`
    : seconds >= 60 ? `${Math.floor(seconds / 60)}분 ${seconds % 60}초` : `${seconds}초`;
}
export function taskView(task, allTasks) {
  const dependencies = task.depends_on || [];
  let status;
  if (task.status === 'done') status = 'completed';
  else if (task.status === 'pending' && dependencies.some(id => !allTasks.some(t => t.id === id && t.status === 'done'))) status = 'waiting_dependency';
  else if (task.status === 'held') status = 'routing_hold';
  else if (['blocked', 'failed', 'cancelled'].includes(task.status)) status = 'failed';
  else if (task.status === 'pending' || task.status === 'prepared') status = 'ready';
  else status = 'running';
  return { id: task.id, title: task.objective || task.id, status,
    statusLabel: status === 'waiting_dependency' ? label(status) : label(task.status),
    glyph: status === 'completed' ? '✓' : status === 'running' ? '●' : status === 'failed' ? '×' : '○',
    difficultyLabel: ROLES[task.role] || task.role || '작업', dependsOn: dependencies };
}
export function graphModel(tasks) {
  const nodes = tasks.map(task => taskView(task, tasks));
  const byId = new Map(nodes.map(node => [node.id, node]));
  const edges = nodes.flatMap(node => node.dependsOn.map(from => ({ from, to: node.id,
    state: byId.get(from)?.status === 'completed' ? 'satisfied' : byId.get(from)?.status === 'failed' ? 'blocked' : 'pending' })));
  return { nodes, edges };
}
export function streamsFor(activity, taskId) {
  return (activity?.streams || []).filter(stream => (stream.task_id || null) === taskId)
    .sort((a, b) => Number(b.process_alive === true) - Number(a.process_alive === true)
      || (b.last_output_at_ms || 0) - (a.last_output_at_ms || 0));
}
export function waitReason(task, tasks) {
  const code = task.wait_reason?.code;
  if (code === 'parallel_limit') return `동시 작업 ${task.wait_reason.limit || ''}개 한도 · 실행 슬롯 대기`;
  if (code === 'resource_capacity') return '해당 CLI의 사용 가능한 실행 슬롯을 기다립니다.';
  if (code === 'write_conflict') return `${task.wait_reason.task_id || '다른 작업'}과 파일 작업 범위가 겹쳐 기다립니다.`;
  const pending = (task.depends_on || []).filter(id => !tasks.some(t => t.id === id && t.status === 'done'));
  if (pending.length) return `선행 작업 대기 · ${pending.join(', ')}`;
  const reason = [...(task.evidence || [])].reverse().find(entry => entry.summary)?.summary;
  return reason || (task.provider ? `역할: ${ROLES[task.role] || task.role || '작업'} · ${task.provider}에 배정`
    : label(task.status));
}
export function outputChunks(stream) {
  return (stream.entries || []).map((entry, index) => ({ id: entry.id || `${stream.id}:${index}`,
    attemptId: stream.id, seq: index + 1, stream: entry.source || 'stdout',
    text: String(entry.text ?? '') + '\n', truncated: Boolean(stream.truncated && index === 0),
    timestamp: stream.last_output_at_ms }));
}
export function artifactOrder(files) {
  const rank = file => file.preview ? /(^|\/)index\.html?$/i.test(file.path) ? 3 : /(^|\/)evidence\//.test(file.path) ? 1 : 2 : 0;
  return [...files].sort((a, b) => rank(b) - rank(a) || a.path.localeCompare(b.path));
}
