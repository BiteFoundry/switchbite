import { createGraph } from './graph.js';
import { createOutputView } from './worker-output.js';
import { ACTIVE, label, pathLabel, duration, graphModel, taskView, streamsFor, waitReason, outputChunks, artifactOrder, usableToolIds, failureText } from './model.js';

const { token, appMode, publicPreview = false, publicApi, publicInfo } = globalThis.switchbite;
const $ = id => document.getElementById(id);
const root = $('wim-app');
const graph = createGraph($('wim-graph-mount'), { eventTarget: root });
const cards = new Map(), outputCache = new Map();
let info = null, state = { status: 'idle', tasks: [], progress: {} }, activity = { streams: [] };
let selectedTools = new Set(), selectedTask = null, composing = true;
let busy = false, polling = false, formDirty = false, currentRun = null, resultKey = null;
let coordinatorOutput = null, coordinatorOutputId = null, recoveryKey = null;
let selectedModel = null;
const setText = (node, text) => { const value = String(text ?? ''); if (node.textContent !== value) node.textContent = value; };
const element = (tag, className, text) => { const node = document.createElement(tag); if (className) node.className = className; if (text != null) node.textContent = text; return node; };

async function api(path, body) {
  if (publicPreview) return publicApi(path, body);
  const response = await fetch(path, { method: body === undefined ? 'GET' : 'POST', cache: 'no-store',
    headers: { 'X-Switchbite-Session': token, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
  const data = await response.json();
  if (!response.ok) throw new Error(data.error?.message || data.error?.code || data.error || '요청을 처리하지 못했습니다. 다시 시도해 주세요.');
  return data;
}
function notice(text) {
  const inForm = !$('wim-start-form').hidden;
  setText($('wim-start-error'), inForm ? text : '');
  setText($('wim-ui-message'), inForm ? '' : text);
  $('wim-ui-message').hidden = inForm || !text;
}
function connection(ok) {
  setText($('wim-connection'), publicPreview ? '공개 미리보기' : ok ? 'Live' : '재연결 중');
  $('wim-connection').dataset.connected = publicPreview ? 'preview' : String(ok);
}
function setBusy(value) {
  busy = value;
  $('wim-btn-start').disabled = value || !usableToolIds(info?.tools, selectedTools).length || !validCoordinators().length;
  for (const id of ['wim-btn-pause', 'wim-btn-resume', 'wim-btn-cancel']) $(id).disabled = publicPreview || value || !state.run_id || !ACTIVE.has(state.status);
  $('wim-tools-refresh').disabled = value;
  for (const input of $('wim-tools-list').querySelectorAll('input')) input.disabled = value || input.dataset.selectable !== 'true';
}
function validCoordinators() { const usable = usableToolIds(info?.tools, selectedTools); return (info?.coordinators || []).filter(candidate => usable.includes(candidate.provider)); }
function coordinatorOptions() {
  const select = $('wim-input-coordinator'), previous = selectedModel || select.value;
  const candidates = validCoordinators();
  const signature = candidates.map(c => c.id).join('|');
  if (select.dataset.signature !== signature) {
    select.replaceChildren(...candidates.map(candidate => { const option = element('option', '', candidate.label); option.value = candidate.id; return option; }));
    select.dataset.signature = signature;
  }
  if (candidates.some(c => c.id === previous)) select.value = previous;
  selectedModel = select.value;
  setBusy(busy);
}
function renderTools() {
  const list = $('wim-tools-list'); list.replaceChildren();
  for (const tool of info?.tools || []) {
    const row = element('label', 'wim-field__check');
    const input = element('input'); input.type = 'checkbox'; input.name = 'tools'; input.value = tool.id;
    input.checked = selectedTools.has(tool.id); input.disabled = !tool.selectable || busy;
    input.dataset.selectable = String(Boolean(tool.selectable));
    const words = element('span'); words.append(element('strong', '', tool.name),
      element('small', '', `${tool.status?.label || ''} · 모델 ${tool.model_count || 0}개`));
    row.append(input, words); list.append(row);
    input.addEventListener('change', async () => {
      if (input.checked) selectedTools.add(tool.id); else selectedTools.delete(tool.id);
      coordinatorOptions(); renderPool();
      setText($('wim-tools-status'), `도구 ${selectedTools.size}개 선택`);
      setBusy(true);
      try { const data = await api('/api/tools', { tools: [...selectedTools] }); info = data; }
      catch (error) { notice(error.message); }
      finally { setBusy(false); }
    });
  }
  setText($('wim-tools-status'), `도구 ${selectedTools.size}개 선택 · ${info?.judge || '판단 설정 확인 중'}`);
  coordinatorOptions();
}
function applyInfo(data, first = false) {
  info = data;
  if (first) {
    selectedTools = new Set(data.selected_tools || []);
    $('wim-input-workspace').value = pathLabel(data.workspace);
  }
  renderTools(); renderPool();
}
function renderPool() {
  const list = $('wim-pool-list');
  const signature = JSON.stringify([(info?.tools || []).map(t => [t.id, t.model_count, t.selectable]), [...selectedTools],
    (state.tasks || []).map(t => [t.id, t.status, t.provider, t.model])]);
  if (list.dataset.signature === signature) return; list.dataset.signature = signature;
  list.replaceChildren();
  for (const tool of info?.tools || []) {
    if (!selectedTools.has(tool.id)) continue;
    const tasks = (state.tasks || []).filter(t => t.provider === tool.id && !['done', 'blocked', 'cancelled'].includes(t.status));
    const row = $('wim-tpl-pool-row').content.firstElementChild.cloneNode(true);
    row.dataset.availability = !tool.selectable ? 'unavailable' : tasks.length ? 'busy' : 'available';
    row.querySelector('[data-slot="provider"]').textContent = tool.name;
    row.querySelector('[data-slot="model"]').textContent = `${tool.model_count || 0}개 모델`;
    row.querySelector('[data-slot="tier"]').textContent = '';
    row.querySelector('[data-slot="availability"]').textContent = !tool.selectable ? (tool.status?.label || '사용 불가') : tasks.length ? `${tasks.length}개 배정` : '사용 가능';
    row.querySelector('[data-slot="assignment"]').textContent = [...new Set(tasks.map(t => t.model).filter(Boolean))].join(' · ');
    list.append(row);
  }
}
function renderRuntime(update) {
  if (publicPreview) { setText($('wim-runtime'), '공개 미리보기 · 예시 작업과 출력'); setText($('wim-engine-updates'), '실제 CLI 실행은 로컬 앱에서 진행합니다.'); return; }
  const runtime = state.runtime || {}, backend = runtime.backend || {};
  setText($('wim-runtime'), [info?.app_version ? `SwitchBite ${info.app_version}` : 'SwitchBite',
    backend.version ? `현재 작업 엔진 ${backend.version}` : '',
    state.run_id ? state.run_id : '새 작업 준비'].filter(Boolean).join(' · '));
  if (!update?.enabled) { setText($('wim-engine-updates'), ''); return; }
  const phases = { checking: '최신 확인 중', fetching_source: '소스 읽는 중', building: '엔진 빌드 중', testing: '엔진 검증 중', current: '자동 동기화', error: '갱신 재시도 대기' };
  setText($('wim-engine-updates'), [update.offline ? '오프라인' : (phases[update.status] || update.status),
    update.ready_version ? `다음 작업 엔진 ${update.ready_version}` : '',
    update.latest_version ? `최신 ${update.latest_version}` : '',
    update.offline ? '' : `${update.interval_seconds}초마다 확인`, update.error || ''].filter(Boolean).join(' · '));
}
function streamChoice(taskId, preferred) {
  const streams = streamsFor(activity, taskId);
  return streams.find(stream => stream.id === preferred) || streams[0] || null;
}
function syncOutput(view, stream) {
  const selected = stream && outputCache.get(stream.id);
  if (selected) view.sync(outputChunks(selected));
}
function renderCoordinator() {
  const coordinator = state.coordinator || {};
  setText($('wim-coordinator-identity'), [coordinator.provider, coordinator.model, coordinator.effort].filter(Boolean).join(' / ')
    || (validCoordinators().find(c => c.id === $('wim-input-coordinator').value)?.label || '조율자를 선택하세요'));
  const stream = streamChoice(null);
  setText($('wim-coordinator-status'), failureText(coordinator.error)
    || (coordinator.status === 'repairing_plan' ? '기존 계획의 형식을 수정하고 있습니다.' : '')
    || (state.status === 'completed' ? '계획과 실행 조율을 마쳤습니다.' : state.status === 'cancelled' ? '작업이 취소되었습니다.' : '')
    || (stream ? `${label(stream.health)} · ${duration(stream.elapsed_ms)}` : state.run_id ? label(coordinator.status || state.status) : '목표를 입력하면 작업을 나누고 담당자를 배정합니다.'));
  const plan = $('wim-coordinator-plan');
  const signature = JSON.stringify((state.tasks || []).map(t => [t.id, t.objective, t.status]));
  if (plan.dataset.signature !== signature) {
    plan.dataset.signature = signature;
    plan.replaceChildren(...(state.tasks || []).map(task => {
      const item = element('li', 'wim-plan__item', `${task.id} · ${task.objective}`); item.title = task.objective;
      return item;
    }));
  }
  if (stream && coordinatorOutputId !== stream.id) {
    coordinatorOutput?.destroy(); coordinatorOutputId = stream.id;
    coordinatorOutput = createOutputView($('wim-coordinator-output'), { attemptId: stream.id, label: '조율자', eventTarget: root });
  }
  if (coordinatorOutput) syncOutput(coordinatorOutput, stream);
}
function selectTask(id, scroll = false) {
  selectedTask = id; graph.setSelected(id);
  for (const [taskId, card] of cards) card.node.dataset.selected = String(taskId === id);
  if (id && cards.has(id)) {
    const card = cards.get(id); expandCard(card, true);
    if (scroll) card.node.scrollIntoView({ block: 'nearest', behavior: 'auto' });
    poll();
  }
}
function expandCard(card, expanded) {
  card.expanded = expanded; card.body.hidden = !expanded;
  card.node.dataset.expanded = String(expanded);
  card.toggle.setAttribute('aria-expanded', String(expanded));
}
function createCard(task) {
  const node = $('wim-tpl-worker-card').content.firstElementChild.cloneNode(true);
  node.dataset.taskId = task.id;
  const slot = key => node.querySelector(`[data-slot="${key}"]`);
  const card = { node, id: task.id, expanded: false, toggle: node.querySelector('.wim-card__toggle'), body: slot('body'),
    slot, view: null, viewId: null, selectedStream: null };
  card.toggle.addEventListener('click', () => {
    const expanded = !card.expanded; expandCard(card, expanded);
    if (expanded) selectTask(task.id); else if (selectedTask === task.id) selectTask(null);
  });
  node.querySelector('[data-action="retry"]').addEventListener('click', () => retryTask(task.id));
  $('wim-worker-list').append(node); cards.set(task.id, card); return card;
}
function facts(mount, values) {
  const signature = JSON.stringify(values); if (mount.dataset.signature === signature) return;
  mount.dataset.signature = signature;
  mount.replaceChildren(...values.flatMap(([title, value]) => [element('dt', '', title), element('dd', 'wim-path', value || '—')]));
}
function renderCards() {
  const tasks = state.tasks || [], keep = new Set(tasks.map(t => t.id));
  for (const [id, card] of cards) if (!keep.has(id)) { card.view?.destroy(); card.node.remove(); cards.delete(id); }
  for (const task of tasks) {
    const card = cards.get(task.id) || createCard(task), model = taskView(task, tasks);
    const streams = streamsFor(activity, task.id), stream = streamChoice(task.id, card.selectedStream);
    card.node.dataset.status = model.status;
    const status = card.slot('status'); status.dataset.status = model.status;
    setText(status.querySelector('.wim-status__glyph'), model.glyph);
    setText(status.querySelector('.wim-status__label'), model.statusLabel);
    setText(card.slot('title'), model.title); setText(card.slot('task-id'), task.id);
    setText(card.slot('difficulty'), model.difficultyLabel);
    setText(card.slot('model'), [task.provider, task.model, task.effort].filter(Boolean).join(' / ') || '담당 모델 배정 대기');
    setText(card.slot('elapsed'), stream ? `${label(stream.health)} · ${duration(stream.elapsed_ms)}` : '');
    setText(card.slot('reason'), waitReason(task, tasks));
    setText(card.slot('preview'), stream?.recent_output || (task.status === 'done' ? '결과 검사와 통합을 마쳤습니다.' : '첫 출력을 기다립니다.'));
    facts(card.slot('assignment'), [ ['배정 모델', [task.provider, task.model, task.effort].filter(Boolean).join(' / ')],
      ['실제 보고 모델', [task.observed?.model, task.observed?.effort].filter(Boolean).join(' / ') || 'CLI에서 아직 보고하지 않음'],
      ['선행 작업', (task.depends_on || []).join(', ') || '없음'], ['산출물', (task.outputs || []).join(', ')],
      ['마지막 출력', stream?.last_output_at_ms ? new Date(stream.last_output_at_ms).toLocaleTimeString('ko-KR') : '아직 없음'] ]);
    const tabs = card.slot('attempts'), signature = JSON.stringify(streams.map(s => [s.id, s.stage, s.status]));
    if (tabs.dataset.signature !== signature) {
      tabs.dataset.signature = signature; tabs.replaceChildren(...streams.map((attempt, index) => {
        const button = element('button', 'wim-btn wim-attempt-tab', `${label(attempt.stage)} ${index + 1}`); button.type = 'button'; button.dataset.streamId = attempt.id;
        button.addEventListener('click', () => { card.selectedStream = attempt.id; renderCards(); poll(); }); return button;
      }));
    }
    for (const button of tabs.children) button.setAttribute('aria-pressed', String(button.dataset.streamId === stream?.id));
    if (card.expanded && stream) {
      if (card.viewId !== stream.id) {
        card.view?.destroy(); card.viewId = stream.id;
        card.view = createOutputView(card.slot('output'), { attemptId: stream.id, label: task.id, eventTarget: root });
      }
      syncOutput(card.view, stream);
    }
    const retry = card.node.querySelector('[data-action="retry"]');
    retry.hidden = !appMode || !streams.some(s => s.can_retry); retry.disabled = busy;
  }
  $('wim-workers-empty').hidden = tasks.length > 0;
  setText($('wim-workers-title'), `작업자${tasks.length ? ` · ${tasks.length}개 작업` : ''}`);
}
function resetViews() {
  for (const card of cards.values()) { card.view?.destroy(); card.node.remove(); }
  cards.clear(); outputCache.clear(); coordinatorOutput?.destroy(); coordinatorOutput = null; coordinatorOutputId = null;
  selectedTask = null; graph.setSelected(null); resultKey = null;
}
function render() {
  if (currentRun !== (state.run_id || null)) { currentRun = state.run_id || null; resetViews(); }
  const active = ACTIVE.has(state.status), tasks = state.tasks || [];
  const status = activity.run_id === state.run_id && ['blocked', 'needs_attention', 'needs_input'].includes(activity.health?.status) ? activity.health.status : state.status;
  root.dataset.runStatus = status;
  const labelNode = $('wim-run-status').querySelector('.wim-status__label');
  $('wim-run-status').dataset.status = status; setText(labelNode, label(status));
  setText($('wim-run-status').querySelector('.wim-status__glyph'), status === 'completed' ? '✓' : ACTIVE.has(status) ? '●' : '–');
  const done = state.progress?.done || 0, total = state.progress?.total || 0;
  $('wim-run-progress').max = Math.max(1, total); $('wim-run-progress').value = done;
  setText($('wim-run-progress-label'), `완료 ${done}/${total}`);
  const running = new Set((activity.streams || []).filter(s => s.task_id && s.process_alive === true).map(s => s.task_id)).size;
  setText($('wim-run-clock'), running); setText($('wim-workspace-tail'), pathLabel(state.workspace || info?.workspace));
  $('wim-btn-pause').hidden = !active || state.status === 'paused'; $('wim-btn-resume').hidden = state.status !== 'paused' && !state.attention;
  $('wim-btn-cancel').hidden = !active;
  $('wim-btn-reset').hidden = !appMode || !state.run_id || active;
  setText($('wim-btn-reset'), composing ? '완료된 작업 보기' : '새 작업');
  $('wim-btn-results').hidden = state.status !== 'completed' || !state.result || composing;
  $('wim-start-form').hidden = !appMode || !composing || active;
  $('wim-context').hidden = !state.run_id || composing;
  $('wim-tools-refresh').hidden = !appMode;
  setText($('wim-context-workspace'), pathLabel(state.workspace)); setText($('wim-context-goal'), state.goal || '');
  setText($('wim-context-tools'), (state.tools || []).join(', ')); setText($('wim-context-parallel'), state.max_parallel || '—');
  $('wim-parallel-form').hidden = !appMode || !['running', 'paused'].includes(state.status);
  if (document.activeElement !== $('wim-live-parallel')) $('wim-live-parallel').value = state.max_parallel || 3;
  const problem = state.attention || state.coordinator?.error;
  $('wim-attention').hidden = !problem;
  setText($('wim-attention-message'), failureText(problem));
  if (state.startup_submission && recoveryKey !== JSON.stringify(state.startup_submission)) {
    const submission = state.startup_submission; recoveryKey = JSON.stringify(submission); composing = true;
    $('wim-input-workspace').value = pathLabel(submission.workspace); $('wim-input-goal').value = submission.goal || '';
    $('wim-input-parallel').value = submission.max_parallel || 3;
    selectedTools = new Set(submission.tools || []); selectedModel = submission.coordinator_id; renderTools();
    $('wim-start-form').hidden = false; goalBytes();
  }
  graph.update(graphModel(tasks)); renderCards(); renderCoordinator(); renderPool(); renderResult(); setBusy(busy);
}
function renderResult() {
  const result = state.result, visible = state.status === 'completed' && result && !composing;
  $('wim-result').hidden = !visible; if (!visible) return;
  const signature = JSON.stringify([state.run_id, result]); if (resultKey === signature) return; resultKey = signature;
  const mount = $('wim-result-body'); mount.replaceChildren();
  const details = element('dl', 'wim-result__facts');
  facts(details, [['작업 경로', pathLabel(result.workspace)], ['브랜치', result.branch], ['커밋', result.commit]]); mount.append(details);
  const copy = element('button', 'wim-btn', '결과 정보 복사'); copy.type = 'button';
  copy.addEventListener('click', async () => { try { await navigator.clipboard.writeText(`작업 경로: ${pathLabel(result.workspace)}\n브랜치: ${result.branch || ''}\n커밋: ${result.commit || ''}`); copy.textContent = '복사했습니다'; } catch { copy.textContent = '경로를 선택해 복사하세요'; } });
  const primary = element('ul', 'wim-artifacts wim-artifacts--primary');
  const more = element('details', 'wim-artifact-details'), summary = element('summary', '', '산출물 확인 중…');
  const files = element('ul', 'wim-artifacts'); more.append(summary, files); mount.append(copy, primary, more);
  api('/api/artifacts').then(data => {
    if (resultKey !== signature) return;
    const ordered = artifactOrder(data.files || []);
    summary.textContent = `전체 산출물 ${ordered.length}개 보기`;
    const main = ordered.find(file => file.preview);
    for (const file of ordered) {
      const row = element('li'), name = element('span', 'wim-path', file.path), action = element('button', 'wim-btn', file.preview ? 'HTML 미리보기' : file.text ? '내용 보기' : '경로 복사'); action.type = 'button';
      row.dataset.artifactPath = file.path;
      if (file === main) action.classList.add('wim-btn--primary');
      action.addEventListener('click', async () => {
        if (file.preview) {
          const tab = window.open('about:blank', '_blank'); if (tab) tab.opener = null;
          try {
            const preview = await api('/api/artifacts/preview', { path: file.path });
            const url = new URL(preview.url);
            if (url.protocol !== 'http:' || url.hostname !== '127.0.0.1') throw Error('미리보기 주소를 확인하지 못했습니다.');
            if (tab) tab.location.replace(url.href); else { const link = element('a', 'wim-btn', '미리보기 열기'); link.href = url.href; link.target = '_blank'; link.rel = 'noopener noreferrer'; row.append(link); }
          } catch (error) { tab?.close(); notice(error.message); }
        } else if (file.text) {
          action.disabled = true;
          try { const data = await api('/api/artifacts/text?path=' + encodeURIComponent(file.path)); let pre = row.querySelector('pre'); if (!pre) { pre = element('pre', 'wim-artifact-text'); row.append(pre); } pre.textContent = data.text; if (data.truncated) notice('파일이 커서 처음 1MiB를 표시합니다. 전체 파일은 결과 경로에서 확인할 수 있습니다.'); }
          catch (error) { notice(error.message); } finally { action.disabled = false; }
        } else { try { await navigator.clipboard.writeText(pathLabel(result.workspace) + '/' + file.path); action.textContent = '복사했습니다'; } catch { notice('파일 경로를 선택해 복사하세요.'); } }
      }); row.append(name, action); (file === main ? primary : files).append(row);
    }
  }).catch(error => { if (resultKey === signature) files.append(element('li', '', error.message)); });
}
async function control(action) {
  if (busy) return; setBusy(true);
  try { state = await api('/api/control', { action }); render(); }
  catch (error) { notice(error.message); } finally { setBusy(false); poll(); }
}
async function retryTask(id) {
  if (busy) return; setBusy(true);
  try { state = await api('/api/task/retry', { task_id: id }); render(); }
  catch (error) { notice(error.message); } finally { setBusy(false); poll(); }
}
function goalBytes() {
  const count = new TextEncoder().encode($('wim-input-goal').value).length;
  setText($('wim-goal-bytes'), `${count.toLocaleString()} / 24,000바이트`); $('wim-goal-bytes').dataset.over = String(count > 24000);
}
async function poll() {
  if (polling) return; polling = true;
  try {
    const values = await Promise.allSettled([api('/api/state'), api('/api/activity'), appMode ? api('/api/runtime') : Promise.resolve(null)]);
    if (values[0].status !== 'fulfilled') throw values[0].reason;
    state = values[0].value;
    if (values[1].status === 'fulfilled' && (!state.run_id || values[1].value.run_id === state.run_id)) activity = values[1].value;
    else activity = { streams: [] };
    if (currentRun !== (state.run_id || null)) { resetViews(); currentRun = state.run_id || null; }
    if (appMode && !info) applyInfo(await api('/api/app'), true);
    if (activity.selected_stream) outputCache.set(activity.selected_stream.id, activity.selected_stream);
    const wanted = new Set();
    const coordinator = streamChoice(null); if (coordinator) wanted.add(coordinator.id);
    for (const card of cards.values()) if (card.expanded) { const stream = streamChoice(card.id, card.selectedStream); if (stream) wanted.add(stream.id); }
    await Promise.allSettled([...wanted].filter(id => id !== activity.selected_stream?.id).map(async id => {
      const data = await api('/api/activity?stream=' + encodeURIComponent(id));
      if (data.run_id === state.run_id && data.selected_stream) outputCache.set(id, data.selected_stream);
    }));
    render(); renderRuntime(values[2].status === 'fulfilled' ? values[2].value : null); connection(true);
  } catch { connection(false); }
  finally { polling = false; }
}

$('wim-input-goal').addEventListener('input', () => { formDirty = true; goalBytes(); });
$('wim-input-coordinator').addEventListener('change', event => { selectedModel = event.target.value; });
$('wim-start-form').addEventListener('submit', async event => {
  event.preventDefault(); if (!appMode || busy || ACTIVE.has(state.status)) return;
  const workspace = $('wim-input-workspace').value.trim(), goal = $('wim-input-goal').value.trim();
  const maxParallel = Number($('wim-input-parallel').value);
  if (!workspace || !goal || new TextEncoder().encode(goal).length > 24000) { notice('프로젝트 경로와 업무를 24,000바이트 이내로 입력하세요.'); return; }
  const tools = usableToolIds(info?.tools, selectedTools);
  if (!tools.length || !validCoordinators().some(c => c.id === $('wim-input-coordinator').value)) { notice('사용할 도구와 조율자를 선택하세요.'); return; }
  if (!Number.isInteger(maxParallel) || maxParallel < 1 || maxParallel > 16) { notice('동시 작업 수는 1~16 사이의 정수로 입력하세요.'); return; }
  setBusy(true); notice('계획과 작업 실행을 시작합니다…');
  try {
    state = await api('/api/start', { workspace, goal, tools, coordinator_id: $('wim-input-coordinator').value, max_parallel: maxParallel });
    if (state.handoff?.url) { const url = new URL(state.handoff.url); if (url.protocol !== 'http:' || url.hostname !== '127.0.0.1') throw Error('새 앱 주소를 확인하지 못했습니다.'); location.assign(url.href); return; }
    composing = false; formDirty = false; notice(''); render();
  } catch (error) { notice(error.message); }
  finally { setBusy(false); poll(); }
});
$('wim-btn-pause').addEventListener('click', () => control('pause'));
$('wim-btn-resume').addEventListener('click', () => control('resume'));
$('wim-btn-cancel').addEventListener('click', () => control('cancel'));
$('wim-btn-reset').addEventListener('click', () => { composing = !composing; if (composing && !formDirty) $('wim-input-goal').value = ''; notice(''); render(); goalBytes(); if (composing) $('wim-input-goal').focus(); });
$('wim-btn-results').addEventListener('click', () => { $('wim-result').scrollIntoView({ block: 'start' }); $('wim-result-title').focus({ preventScroll: true }); });
$('wim-btn-attention').addEventListener('click', () => { if (state.status === 'idle') $('wim-start-form').requestSubmit(); else control('resume'); });
$('wim-btn-goal-more').addEventListener('click', () => { const goal = $('wim-context-goal'), expanded = goal.dataset.expanded !== 'true'; goal.dataset.expanded = String(expanded); $('wim-btn-goal-more').setAttribute('aria-expanded', String(expanded)); $('wim-btn-goal-more').textContent = expanded ? '접기' : '전체 보기'; });
$('wim-tools-refresh').addEventListener('click', async () => {
  if (busy) return; setBusy(true); setText($('wim-tools-status'), 'CLI와 모델을 다시 확인합니다…');
  try { applyInfo(await api('/api/tools/refresh', {})); notice(''); }
  catch (error) { notice(error.message); } finally { setBusy(false); }
});
$('wim-parallel-form').addEventListener('submit', async event => {
  event.preventDefault(); if (busy) return; setBusy(true);
  try { state = await api('/api/parallelism', { max_parallel: Number($('wim-live-parallel').value) }); render(); }
  catch (error) { notice(error.message); } finally { setBusy(false); }
});
root.addEventListener('wim:select-task', event => selectTask(event.detail.taskId, true));
document.addEventListener('selectionchange', () => { if (!document.getSelection()?.isCollapsed) return; renderCards(); renderCoordinator(); });

async function init() {
  try {
    if (publicPreview) applyInfo(publicInfo, true);
    if (appMode) { const data = await api('/api/app'); applyInfo(data, true); state = data.current || state; composing = !data.current || data.current.status === 'idle'; }
    else { composing = false; $('wim-start-form').hidden = true; }
    if (!formDirty) goalBytes(); render(); await poll();
  } catch (error) { connection(false); notice(error.message); }
}
init(); setInterval(poll, 2000);
